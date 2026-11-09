//! 加密原语 —— 消息摘要、对称加密、随机数与 HMAC。
//!
//! 为 Hib OS 用户态宏内核提供轻量加密能力，供沙箱、审计、动态链接器
//! 与安全子系统使用。全部为纯 Rust 实现，无外部 crate 依赖，可脱网运行。
//!
//! 设计要点:
//! * `CryptoHash`：`sha256`（真实 FIPS 180-4 实现，可校验标准向量）、
//!   `sha512`（结构化简化版，基于 f64 圆常数，确定性）、`md5`（真实 RFC 1321
//!   实现）；
//! * `CryptoCipher`：`aes_encrypt`/`aes_decrypt`（基于 SHA-256 密钥流的简化对称
//!   加密，加密与解密互为逆运算）、`xor_crypt`（循环密钥异或）；
//! * `CryptoRandom`：`random_bytes`/`random_u64`/`random_uuid`（系统时间 + 计数器
//!   PRNG 简化版，不适用于生产环境）；
//! * `CryptoHmac`：`hmac_sha256`（真实 HMAC 结构，基于内置 sha256）；
//! * Base64 编码/解码简化版。
//!
//! ⚠️ 诚实边界：本模块为原型用途，`sha512`、`random_*` 与 `aes_*` 均为简化版，
//! 不满足生产环境安全要求，生产请替换为成熟密码学库。
//!
//! 文件末尾附带 `#[cfg(test)]` 单元测试，覆盖标准向量（SHA-256/MD5/HMAC）、
//! 对称加密往返、异或、随机数确定性、Base64 往返等。

// ========================================================================
// SHA-256（真实 FIPS 180-4 实现）
// ========================================================================

/// SHA-256 摘要，32 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256Digest(pub [u8; 32]);

/// SHA-256 圆常数（立方根小数部分前 32 位，前 64 个素数）。
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 初始哈希值（平方根小数部分前 32 位，前 8 个素数）。
const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn rotr32(x: u32, n: u32) -> u32 {
    x.rotate_right(n)
}

fn shr32(x: u32, n: u32) -> u32 {
    x >> n
}

fn sigma0_256(x: u32) -> u32 {
    rotr32(x, 7) ^ rotr32(x, 18) ^ shr32(x, 3)
}

fn sigma1_256(x: u32) -> u32 {
    rotr32(x, 17) ^ rotr32(x, 19) ^ shr32(x, 10)
}

fn big_sigma0_256(x: u32) -> u32 {
    rotr32(x, 2) ^ rotr32(x, 13) ^ rotr32(x, 22)
}

fn big_sigma1_256(x: u32) -> u32 {
    rotr32(x, 6) ^ rotr32(x, 11) ^ rotr32(x, 25)
}

fn choose_256(e: u32, f: u32, g: u32) -> u32 {
    (e & f) ^ ((!e) & g)
}

fn majority_256(a: u32, b: u32, c: u32) -> u32 {
    (a & b) ^ (a & c) ^ (b & c)
}

fn sha256_transform(state: &mut [u32; 8], block: &[u8]) {
    assert_eq!(block.len(), 64);
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = sigma0_256(w[i - 15]);
        let s1 = sigma1_256(w[i - 2]);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];
    for i in 0..64 {
        let s1 = big_sigma1_256(e);
        let ch = choose_256(e, f, g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = big_sigma0_256(a);
        let maj = majority_256(a, b, c);
        let temp2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// 计算 SHA-256 摘要。
pub fn sha256(data: &[u8]) -> Sha256Digest {
    let mut state = SHA256_H0;
    let len = data.len();
    let mut pos = 0;
    while pos + 64 <= len {
        sha256_transform(&mut state, &data[pos..pos + 64]);
        pos += 64;
    }
    // 填充。
    let mut buf = [0u8; 64];
    let remain = len - pos;
    buf[..remain].copy_from_slice(&data[pos..]);
    buf[remain] = 0x80;
    if remain + 1 > 56 {
        sha256_transform(&mut state, &buf);
        buf = [0u8; 64];
    }
    // 64 位大端位长（比特）。
    let bitlen = (len as u64).wrapping_mul(8);
    buf[56..64].copy_from(&bitlen.to_be_bytes());
    sha256_transform(&mut state, &buf);
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from(&state[i].to_be_bytes());
    }
    Sha256Digest(out)
}

impl Sha256Digest {
    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ========================================================================
// MD5（真实 RFC 1321 实现）
// ========================================================================

/// MD5 摘要，16 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Md5Digest(pub [u8; 16]);

fn left_rotate32(x: u32, c: u32) -> u32 {
    x.rotate_left(c)
}

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
    20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10,
    15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn md5_k(i: usize) -> u32 {
    // floor(2^32 * abs(sin(i + 1)))
    (4294967296.0_f64 * ((i as f64 + 1.0).sin().abs())) as u32
}

fn md5_transform(state: &mut [u32; 4], block: &[u8]) {
    assert_eq!(block.len(), 64);
    let mut m = [0u32; 16];
    for i in 0..16 {
        m[i] = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    for i in 0..64 {
        let (f, g) = match i {
            0..=15 => {
                let f = (b & c) | ((!b) & d);
                let g = i as u32;
                (f, g)
            }
            16..=31 => {
                let f = (d & b) | ((!d) & c);
                let g = (5 * i as u32 + 1) % 16;
                (f, g)
            }
            32..=47 => {
                let f = b ^ c ^ d;
                let g = (3 * i as u32 + 5) % 16;
                (f, g)
            }
            _ => {
                let f = c ^ (b | (!d));
                let g = (7 * i as u32) % 16;
                (f, g)
            }
        };
        let temp = d;
        d = c;
        c = b;
        b = b.wrapping_add(left_rotate32(
            a.wrapping_add(f).wrapping_add(md5_k(i)).wrapping_add(m[g as usize]),
            MD5_S[i],
        ));
        a = temp;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

/// 计算 MD5 摘要。
pub fn md5(data: &[u8]) -> Md5Digest {
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let len = data.len();
    let mut pos = 0;
    while pos + 64 <= len {
        md5_transform(&mut state, &data[pos..pos + 64]);
        pos += 64;
    }
    let mut buf = [0u8; 64];
    let remain = len - pos;
    buf[..remain].copy_from_slice(&data[pos..]);
    buf[remain] = 0x80;
    if remain + 1 > 56 {
        md5_transform(&mut state, &buf);
        buf = [0u8; 64];
    }
    let bitlen = (len as u64).wrapping_mul(8) as u32;
    buf[56..60].copy_from(&bitlen.to_le_bytes());
    buf[60..64].copy_from(&0u32.to_le_bytes());
    md5_transform(&mut state, &buf);
    let mut out = [0u8; 16];
    for i in 0..4 {
        out[i * 4..i * 4 + 4].copy_from(&state[i].to_le_bytes());
    }
    Md5Digest(out)
}

impl Md5Digest {
    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ========================================================================
// SHA-512（结构化简化版，确定性）
// ========================================================================

/// SHA-512 摘要，64 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha512Digest(pub [u8; 64]);

const SHA512_H0: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

/// SHA-512 圆常数（基于立方根的近似，f64 计算，简化版不完全等价于 FIPS 180-4）。
fn sha512_k(i: usize) -> u64 {
    // 取前 80 个素数的立方根小数部分，放大到 64 位。
    let primes: [u64; 80] = [
        2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89,
        97, 101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191,
        193, 197, 199, 211, 223, 227, 229, 233, 239, 241, 251, 257, 263, 269, 271, 277, 281, 283, 293,
        307, 311, 313, 317, 331, 337, 347, 349, 353, 359, 367, 373, 379, 383, 389, 397, 401, 409, 419,
        421, 431, 433, 439, 443, 449, 457, 461, 463, 467, 479, 487, 491, 499, 503, 509, 521, 523, 541,
    ];
    let p = primes[i] as f64;
    let cbrt = p.cbrt();
    let frac = cbrt - cbrt.trunc();
    (frac * 18446744073709551616.0_f64) as u64
}

fn rotr64(x: u64, n: u32) -> u64 {
    x.rotate_right(n)
}

fn shr64(x: u64, n: u32) -> u64 {
    x >> n
}

fn sigma0_512(x: u64) -> u64 {
    rotr64(x, 1) ^ rotr64(x, 8) ^ shr64(x, 7)
}

fn sigma1_512(x: u64) -> u64 {
    rotr64(x, 19) ^ rotr64(x, 61) ^ shr64(x, 6)
}

fn big_sigma0_512(x: u64) -> u64 {
    rotr64(x, 28) ^ rotr64(x, 34) ^ rotr64(x, 39)
}

fn big_sigma1_512(x: u64) -> u64 {
    rotr64(x, 14) ^ rotr64(x, 18) ^ rotr64(x, 41)
}

fn choose_512(e: u64, f: u64, g: u64) -> u64 {
    (e & f) ^ ((!e) & g)
}

fn majority_512(a: u64, b: u64, c: u64) -> u64 {
    (a & b) ^ (a & c) ^ (b & c)
}

fn sha512_transform(state: &mut [u64; 8], block: &[u8]) {
    assert_eq!(block.len(), 128);
    let mut w = [0u64; 80];
    for i in 0..16 {
        w[i] = u64::from_be_bytes([
            block[i * 8], block[i * 8 + 1], block[i * 8 + 2], block[i * 8 + 3],
            block[i * 8 + 4], block[i * 8 + 5], block[i * 8 + 6], block[i * 8 + 7],
        ]);
    }
    for i in 16..80 {
        let s0 = sigma0_512(w[i - 15]);
        let s1 = sigma1_512(w[i - 2]);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];
    for i in 0..80 {
        let s1 = big_sigma1_512(e);
        let ch = choose_512(e, f, g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(sha512_k(i))
            .wrapping_add(w[i]);
        let s0 = big_sigma0_512(a);
        let maj = majority_512(a, b, c);
        let temp2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// 计算 SHA-512 摘要（简化版，确定性）。
pub fn sha512(data: &[u8]) -> Sha512Digest {
    let mut state = SHA512_H0;
    let len = data.len();
    let mut pos = 0;
    while pos + 128 <= len {
        sha512_transform(&mut state, &data[pos..pos + 128]);
        pos += 128;
    }
    let mut buf = [0u8; 128];
    let remain = len - pos;
    buf[..remain].copy_from_slice(&data[pos..]);
    buf[remain] = 0x80;
    if remain + 1 > 112 {
        sha512_transform(&mut state, &buf);
        buf = [0u8; 128];
    }
    let bitlen = (len as u128).wrapping_mul(8);
    buf[112..120].copy_from(&((bitlen >> 64) as u64).to_be_bytes());
    buf[120..128].copy_from(&(bitlen as u64).to_be_bytes());
    sha512_transform(&mut state, &buf);
    let mut out = [0u8; 64];
    for i in 0..8 {
        out[i * 8..i * 8 + 8].copy_from(&state[i].to_be_bytes());
    }
    Sha512Digest(out)
}

impl Sha512Digest {
    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ========================================================================
// 对称加密
// ========================================================================

/// 对称加密/解密（简化版：以 SHA-256 扩展密钥为密钥流，异或数据）。
pub struct CryptoCipher;

impl CryptoCipher {
    /// 创建加密器。
    pub fn new() -> Self {
        CryptoCipher
    }

    /// AES 风格加密（简化版）：生成与明文等长的密钥流并异或。
    ///
    /// 与 `aes_decrypt` 互为逆运算（同一密钥）。
    pub fn aes_encrypt(&self, key: &[u8], plaintext: &[u8]) -> Vec<u8> {
        self.xor_stream(key, plaintext)
    }

    /// AES 风格解密（简化版）：与 `aes_encrypt` 相同操作，因异或自逆。
    pub fn aes_decrypt(&self, key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
        self.xor_stream(key, ciphertext)
    }

    /// 循环密钥异或（真实异或，非对称加密，仅用于简单混淆）。
    pub fn xor_crypt(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        if key.is_empty() {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(data.len());
        for (i, &b) in data.iter().enumerate() {
            out.push(b ^ key[i % key.len()]);
        }
        out
    }

    /// 基于密钥与计数器生成密钥流（SHA-256(key || counter) 分块）。
    fn xor_stream(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut counter: u64 = 0;
        let mut pos = 0;
        while pos < data.len() {
            let mut block_input = Vec::with_capacity(key.len() + 8);
            block_input.extend_from_slice(key);
            block_input.extend_from_slice(&counter.to_be_bytes());
            let stream_block = sha256(&block_input).0;
            let take = (data.len() - pos).min(stream_block.len());
            for i in 0..take {
                out.push(data[pos + i] ^ stream_block[i]);
            }
            pos += take;
            counter += 1;
        }
        out
    }
}

impl Default for CryptoCipher {
    fn default() -> Self {
        CryptoCipher::new()
    }
}

// ========================================================================
// 随机数
// ========================================================================

/// 伪随机数生成器（系统时间 + 计数器，简化版，不适用于生产环境）。
pub struct CryptoRandom {
    state: u64,
    counter: u64,
}

impl CryptoRandom {
    /// 创建 PRNG，以当前系统时间为种子。
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(1)
            .wrapping_add(1);
        Self { state: seed, counter: 0 }
    }

    /// 生成 n 字节随机数据。
    pub fn random_bytes(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n);
        let mut pos = 0;
        while pos < n {
            self.state = self
                .state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407)
                .wrapping_add(self.counter);
            self.counter = self.counter.wrapping_add(1);
            let block = self.state.to_be_bytes();
            let take = (n - pos).min(block.len());
            out.extend_from_slice(&block[..take]);
            pos += take;
        }
        out
    }

    /// 生成一个 u64 随机数。
    pub fn random_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            .wrapping_add(self.counter);
        self.counter = self.counter.wrapping_add(1);
        self.state
    }

    /// 生成一个 UUID 字符串（简化版 v4 风格）。
    pub fn random_uuid(&mut self) -> String {
        let b = self.random_bytes(16);
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12],
            b[13], b[14], b[15]
        )
    }
}

impl Default for CryptoRandom {
    fn default() -> Self {
        CryptoRandom::new()
    }
}

// ========================================================================
// HMAC-SHA256（真实 HMAC 结构，基于内置 sha256）
// ========================================================================

/// HMAC-SHA256 摘要，32 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HmacSha256(pub [u8; 32]);

const HMAC_BLOCK_SIZE: usize = 64;

/// 计算 HMAC-SHA256。
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> HmacSha256 {
    let mut k = key.to_vec();
    if k.len() > HMAC_BLOCK_SIZE {
        k = sha256(&k).0.to_vec();
    }
    k.resize(HMAC_BLOCK_SIZE, 0);
    let mut ipad = [0u8; HMAC_BLOCK_SIZE];
    let mut opad = [0u8; HMAC_BLOCK_SIZE];
    for i in 0..HMAC_BLOCK_SIZE {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Vec::with_capacity(HMAC_BLOCK_SIZE + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_digest = sha256(&inner).0;
    let mut outer = Vec::with_capacity(HMAC_BLOCK_SIZE + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_digest);
    let outer_digest = sha256(&outer).0;
    HmacSha256(outer_digest)
}

impl HmacSha256 {
    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ========================================================================
// Base64（简化版）
// ========================================================================

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 编码。
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(BASE64_CHARS[((triple >> 18) & 0x3f) as usize] as char);
        out.push(BASE64_CHARS[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_CHARS[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_CHARS[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Base64 解码。
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim_end_matches('=');
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for ch in input.chars() {
        let val = BASE64_CHARS.iter().position(|&b| b == ch as u8)? as u32;
        buffer = (buffer << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

// ========================================================================
// 命名空间结构（供 `crate::crypto::CryptoHash` 等风格调用）
// ========================================================================

/// 消息摘要命名空间。
pub struct CryptoHash;
impl CryptoHash {
    pub fn sha256(data: &[u8]) -> Sha256Digest {
        sha256(data)
    }
    pub fn sha512(data: &[u8]) -> Sha512Digest {
        sha512(data)
    }
    pub fn md5(data: &[u8]) -> Md5Digest {
        md5(data)
    }
}

/// 对称加密命名空间。
pub struct CryptoCipherNS;
impl CryptoCipherNS {
    pub fn aes_encrypt(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
        CryptoCipher::new().aes_encrypt(key, plaintext)
    }
    pub fn aes_decrypt(key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
        CryptoCipher::new().aes_decrypt(key, ciphertext)
    }
    pub fn xor_crypt(key: &[u8], data: &[u8]) -> Vec<u8> {
        CryptoCipher::new().xor_crypt(key, data)
    }
}

/// 随机数命名空间。
pub struct CryptoRandomNS;
impl CryptoRandomNS {
    pub fn random_bytes(n: usize) -> Vec<u8> {
        CryptoRandom::new().random_bytes(n)
    }
    pub fn random_u64() -> u64 {
        CryptoRandom::new().random_u64()
    }
    pub fn random_uuid() -> String {
        CryptoRandom::new().random_uuid()
    }
}

/// HMAC 命名空间。
pub struct CryptoHmac;
impl CryptoHmac {
    pub fn hmac_sha256(key: &[u8], data: &[u8]) -> HmacSha256 {
        hmac_sha256(key, data)
    }
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_and_abc() {
        assert_eq!(
            sha256(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 确定性。
        assert_eq!(sha256(b"hello"), sha256(b"hello"));
    }

    #[test]
    fn md5_empty_and_abc() {
        assert_eq!(
            md5(b"").to_hex(),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            md5(b"abc").to_hex(),
            "900150983cd24fb0d6968f73ac8e220b"
        );
    }

    #[test]
    fn sha512_deterministic_and_distinct() {
        let a = sha512(b"hello");
        let b = sha512(b"hello");
        let c = sha512(b"hell0");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(sha512(b"").0.len(), 64);
    }

    #[test]
    fn hmac_sha256_vector() {
        let key = b"key";
        let data = b"The quick brown fox jumps over the lazy dog";
        let mac = hmac_sha256(key, data);
        assert_eq!(
            mac.to_hex(),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn aes_roundtrip() {
        let cipher = CryptoCipher::new();
        let key = b"mysecretkey";
        let plaintext = b"Hello, Hib OS encrypted message!";
        let ct = cipher.aes_encrypt(key, plaintext);
        let pt = cipher.aes_decrypt(key, &ct);
        assert_eq!(pt, plaintext);
        // 不同密钥产生不同密文。
        let ct2 = cipher.aes_encrypt(b"otherkey", plaintext);
        assert_ne!(ct, ct2);
    }

    #[test]
    fn xor_crypt() {
        let cipher = CryptoCipher::new();
        let data = b"xor test data";
        let key = b"key";
        let enc = cipher.xor_crypt(key, data);
        let dec = cipher.xor_crypt(key, &enc);
        assert_eq!(dec, data);
        // 空密钥原样返回。
        assert_eq!(cipher.xor_crypt(&[], data), data);
    }

    #[test]
    fn random_deterministic_seed_and_range() {
        // 同一种子应产生同一序列。
        let mut r1 = CryptoRandom { state: 12345, counter: 0 };
        let mut r2 = CryptoRandom { state: 12345, counter: 0 };
        assert_eq!(r1.random_bytes(16), r2.random_bytes(16));
        assert_eq!(r1.random_u64(), r2.random_u64());
        let bytes = CryptoRandom::new().random_bytes(32);
        assert_eq!(bytes.len(), 32);
        let uuid = CryptoRandom::new().random_uuid();
        assert_eq!(uuid.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        let decoded = base64_decode("Zm9vYmFy").unwrap();
        assert_eq!(decoded, b"foobar");
        let decoded_empty = base64_decode("").unwrap();
        assert!(decoded_empty.is_empty());
    }

    #[test]
    fn namespaces() {
        assert_eq!(CryptoHash::sha256(b"abc").to_hex().len(), 64);
        assert_eq!(CryptoHash::md5(b"abc").to_hex().len(), 32);
        assert_eq!(CryptoHash::sha512(b"abc").0.len(), 64);
        let ct = CryptoCipherNS::aes_encrypt(b"k", b"data");
        assert_eq!(CryptoCipherNS::aes_decrypt(b"k", &ct), b"data");
        let _ = CryptoRandomNS::random_bytes(8);
        let _ = CryptoRandomNS::random_u64();
        let _ = CryptoRandomNS::random_uuid();
        let _ = CryptoHmac::hmac_sha256(b"k", b"d");
    }
}
