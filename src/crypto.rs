//! 极简加密和校验 API。
//!
//! 这个模块统一提供常用摘要、HMAC、AES-256-GCM 加密解密、Ed25519 签名和 secp256k1 签名。
//! 它只暴露字节和字符串结果，不暴露底层加密库类型。

use std::{error::Error as StdError, fmt, fs as std_fs};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit as AesKeyInit},
};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{Error as PasswordHashError, SaltString},
};
use ed25519_dalek::{
    Signature as Ed25519Signature, Signer, SigningKey as Ed25519SigningKey, Verifier,
    VerifyingKey as Ed25519VerifyingKey,
};
use hmac::{Hmac, KeyInit as HmacKeyInit, Mac};
use k256::ecdsa::{
    Signature as Secp256k1Signature, SigningKey as Secp256k1SigningKey,
    VerifyingKey as Secp256k1VerifyingKey,
};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};
use sha3::Keccak256;
use subtle::ConstantTimeEq;

use crate::fs::Path as FsPath;

const AES256_KEY_LEN: usize = 32;
const AES256_NONCE_LEN: usize = 12;
const ED25519_SECRET_LEN: usize = 32;
const ED25519_PUBLIC_LEN: usize = 32;
const ED25519_SIGNATURE_LEN: usize = 64;
const SECP256K1_SECRET_LEN: usize = 32;
const PASSWORD_SALT_LEN: usize = 16;

/// crypto 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let key = crypto::aes256_key()?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// crypto 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分文件读取、密钥形状、加密或签名错误时，使用
/// [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + Send + Sync + 'static) -> Self {
        Self {
            kind,
            source: Some(Box::new(source)),
        }
    }

    /// 返回具体错误类型。
    ///
    /// 调用方可以通过匹配 [`ErrorKind`] 做精细错误处理。
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// crypto 模块的具体错误原因。
///
/// 错误信息会包含操作名和关键上下文，方便定位哪个加密、签名或文件读取步骤失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 读取文件失败。
    #[error("crypto {operation} `{path}` failed")]
    Read {
        /// 发生错误的操作名，例如 `file_sha256`。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 随机数生成失败。
    #[error("crypto {operation} random failed")]
    Random {
        /// 发生错误的操作名。
        operation: &'static str,
    },

    /// 输入长度不符合算法要求。
    #[error("crypto {operation} failed: {name} length must be {expected}, got {actual}")]
    Length {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 发生错误的输入名。
        name: &'static str,
        /// 期望长度。
        expected: usize,
        /// 实际长度。
        actual: usize,
    },

    /// AES-256-GCM 加密或解密失败。
    #[error("crypto {operation} failed: {message}")]
    Aes {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 面向人的错误说明。
        message: String,
    },

    /// 签名或验签失败。
    #[error("crypto {operation} failed: {message}")]
    Sign {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 面向人的错误说明。
        message: String,
    },

    /// 密码哈希或验证失败。
    #[error("crypto {operation} password failed: {message}")]
    Password {
        /// 发生错误的操作名，例如 `password_hash`。
        operation: &'static str,
        /// 面向人的错误说明。
        message: String,
    },
}

/// 公钥和私钥字节对。
///
/// 用于 Ed25519 和 secp256k1。公开方法只返回字节切片，避免暴露底层签名库类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyPair {
    public_key: Vec<u8>,
    secret_key: Vec<u8>,
}

impl KeyPair {
    /// 返回公钥字节。
    ///
    /// Ed25519 为 32 字节；secp256k1 为 33 字节压缩公钥。
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// 返回私钥字节。
    ///
    /// 当前支持的 Ed25519 和 secp256k1 私钥都为 32 字节。
    #[must_use]
    pub fn secret_key(&self) -> &[u8] {
        &self.secret_key
    }
}

/// 计算 SHA-256 十六进制摘要。
#[must_use]
pub fn sha256(input: impl AsRef<[u8]>) -> String {
    hex(Sha256::digest(input.as_ref()))
}

/// 计算 SHA-512 十六进制摘要。
#[must_use]
pub fn sha512(input: impl AsRef<[u8]>) -> String {
    hex(Sha512::digest(input.as_ref()))
}

/// 读取文件并计算 SHA-256 十六进制摘要。
pub fn file_sha256(path: impl Into<FsPath>) -> Result<String> {
    let path = path.into();
    let bytes = read_file("file_sha256", &path)?;
    Ok(sha256(bytes))
}

/// 读取文件并计算 SHA-512 十六进制摘要。
pub fn file_sha512(path: impl Into<FsPath>) -> Result<String> {
    let path = path.into();
    let bytes = read_file("file_sha512", &path)?;
    Ok(sha512(bytes))
}

/// 计算 HMAC-SHA256 十六进制摘要。
#[must_use]
pub fn hmac_sha256(key: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> String {
    let Ok(mut mac) = <Hmac<Sha256> as HmacKeyInit>::new_from_slice(key.as_ref()) else {
        return String::new();
    };
    mac.update(data.as_ref());
    hex(mac.finalize().into_bytes())
}

/// 计算 HMAC-SHA512 十六进制摘要。
#[must_use]
pub fn hmac_sha512(key: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> String {
    let Ok(mut mac) = <Hmac<Sha512> as HmacKeyInit>::new_from_slice(key.as_ref()) else {
        return String::new();
    };
    mac.update(data.as_ref());
    hex(mac.finalize().into_bytes())
}

/// 常量时间比较两个字节序列是否相等。
///
/// 适合比较摘要、签名或认证 token，避免普通比较提前返回泄露长度相同输入的差异位置。
#[must_use]
pub fn secure_eq(left: impl AsRef<[u8]>, right: impl AsRef<[u8]>) -> bool {
    let left = left.as_ref();
    let right = right.as_ref();
    left.len() == right.len() && left.ct_eq(right).into()
}

/// 计算 Keccak-256 十六进制摘要。
///
/// 这是以太坊等链上场景常用的 Keccak，不是 NIST SHA3-256。
#[must_use]
pub fn keccak256(input: impl AsRef<[u8]>) -> String {
    hex(Keccak256::digest(input.as_ref()))
}

/// 计算 RIPEMD-160 十六进制摘要。
#[must_use]
pub fn ripemd160(input: impl AsRef<[u8]>) -> String {
    hex(Ripemd160::digest(input.as_ref()))
}

/// 计算 Bitcoin 风格 HASH160：RIPEMD160(SHA256(data))。
#[must_use]
pub fn hash160(input: impl AsRef<[u8]>) -> String {
    let sha = Sha256::digest(input.as_ref());
    ripemd160(sha)
}

/// 生成安全密码哈希字符串。
///
/// 使用 Argon2id 默认参数和内部随机 salt，返回 PHC 字符串，可直接保存到数据库。调用方不需要
/// 自己管理 salt 或算法参数。
pub fn password_hash(password: impl AsRef<[u8]>) -> Result<String> {
    let salt_bytes = random_array::<PASSWORD_SALT_LEN>("password_hash")?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|source| ErrorKind::Password {
        operation: "password_hash",
        message: source.to_string(),
    })?;
    Argon2::default()
        .hash_password(password.as_ref(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|source| {
            ErrorKind::Password {
                operation: "password_hash",
                message: source.to_string(),
            }
            .into()
        })
}

/// 验证密码是否匹配已保存的哈希。
///
/// 密码不匹配返回 `Ok(false)`；哈希字符串格式错误或算法无法处理时返回 [`ErrorKind::Password`]。
pub fn password_verify(password: impl AsRef<[u8]>, hash: impl AsRef<str>) -> Result<bool> {
    let parsed = PasswordHash::new(hash.as_ref()).map_err(|source| ErrorKind::Password {
        operation: "password_verify",
        message: source.to_string(),
    })?;
    match Argon2::default().verify_password(password.as_ref(), &parsed) {
        Ok(()) => Ok(true),
        Err(PasswordHashError::Password) => Ok(false),
        Err(source) => Err(ErrorKind::Password {
            operation: "password_verify",
            message: source.to_string(),
        }
        .into()),
    }
}

/// 生成 32 字节 AES-256 密钥。
pub fn aes256_key() -> Result<Vec<u8>> {
    random_bytes("aes256_key", AES256_KEY_LEN)
}

/// 使用 AES-256-GCM 加密数据。
///
/// 返回值格式固定为 `nonce + ciphertext + tag`，可直接传给 [`aes256_decrypt`]。
pub fn aes256_encrypt(key: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let key = fixed_array::<AES256_KEY_LEN>("aes256_encrypt", "key", key.as_ref())?;
    let nonce = random_array::<AES256_NONCE_LEN>("aes256_encrypt")?;
    let cipher = Aes256Gcm::new(&key.into());
    let mut output = nonce.to_vec();
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), data.as_ref())
        .map_err(|_| ErrorKind::Aes {
            operation: "aes256_encrypt",
            message: "encrypt failed".to_owned(),
        })?;
    output.extend_from_slice(&encrypted);
    Ok(output)
}

/// 使用 AES-256-GCM 解密数据。
///
/// 输入必须是 [`aes256_encrypt`] 返回的 `nonce + ciphertext + tag` 字节。
pub fn aes256_decrypt(key: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let key = fixed_array::<AES256_KEY_LEN>("aes256_decrypt", "key", key.as_ref())?;
    let data = data.as_ref();
    if data.len() < AES256_NONCE_LEN {
        return Err(ErrorKind::Length {
            operation: "aes256_decrypt",
            name: "data",
            expected: AES256_NONCE_LEN,
            actual: data.len(),
        }
        .into());
    }
    let (nonce, encrypted) = data.split_at(AES256_NONCE_LEN);
    let cipher = Aes256Gcm::new(&key.into());
    cipher
        .decrypt(Nonce::from_slice(nonce), encrypted)
        .map_err(|_| {
            ErrorKind::Aes {
                operation: "aes256_decrypt",
                message: "decrypt failed".to_owned(),
            }
            .into()
        })
}

/// 生成 Ed25519 公私钥对。
pub fn ed25519_keypair() -> Result<KeyPair> {
    let secret = random_array::<ED25519_SECRET_LEN>("ed25519_keypair")?;
    let signing = Ed25519SigningKey::from_bytes(&secret);
    let verifying = signing.verifying_key();
    Ok(KeyPair {
        public_key: verifying.to_bytes().to_vec(),
        secret_key: secret.to_vec(),
    })
}

/// 使用 Ed25519 私钥签名数据。
pub fn ed25519_sign(secret: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let secret = fixed_array::<ED25519_SECRET_LEN>("ed25519_sign", "secret", secret.as_ref())?;
    let signing = Ed25519SigningKey::from_bytes(&secret);
    Ok(signing.sign(data.as_ref()).to_bytes().to_vec())
}

/// 使用 Ed25519 公钥验证签名。
pub fn ed25519_verify(
    public: impl AsRef<[u8]>,
    data: impl AsRef<[u8]>,
    signature: impl AsRef<[u8]>,
) -> Result<bool> {
    let public = fixed_array::<ED25519_PUBLIC_LEN>("ed25519_verify", "public", public.as_ref())?;
    let signature =
        fixed_array::<ED25519_SIGNATURE_LEN>("ed25519_verify", "signature", signature.as_ref())?;
    let verifying = Ed25519VerifyingKey::from_bytes(&public).map_err(|_| ErrorKind::Sign {
        operation: "ed25519_verify",
        message: "invalid public key".to_owned(),
    })?;
    let signature = Ed25519Signature::from_bytes(&signature);
    Ok(verifying.verify(data.as_ref(), &signature).is_ok())
}

/// 生成 secp256k1 公私钥对。
pub fn secp256k1_keypair() -> Result<KeyPair> {
    let signing = loop {
        let secret = random_array::<SECP256K1_SECRET_LEN>("secp256k1_keypair")?;
        if let Ok(signing) = Secp256k1SigningKey::from_slice(&secret) {
            break signing;
        }
    };
    let verifying = signing.verifying_key();
    Ok(KeyPair {
        public_key: verifying.to_encoded_point(true).as_bytes().to_vec(),
        secret_key: signing.to_bytes().to_vec(),
    })
}

/// 使用 secp256k1 私钥签名数据。
///
/// 返回 DER 格式签名字节，适合存储和传输。
pub fn secp256k1_sign(secret: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let secret = fixed_array::<SECP256K1_SECRET_LEN>("secp256k1_sign", "secret", secret.as_ref())?;
    let signing = Secp256k1SigningKey::from_slice(&secret).map_err(|_| ErrorKind::Sign {
        operation: "secp256k1_sign",
        message: "invalid secret key".to_owned(),
    })?;
    let signature: Secp256k1Signature = signing.sign(data.as_ref());
    Ok(signature.to_der().as_bytes().to_vec())
}

/// 使用 secp256k1 公钥验证签名。
///
/// 签名必须是 [`secp256k1_sign`] 返回的 DER 格式字节。
pub fn secp256k1_verify(
    public: impl AsRef<[u8]>,
    data: impl AsRef<[u8]>,
    signature: impl AsRef<[u8]>,
) -> Result<bool> {
    let verifying =
        Secp256k1VerifyingKey::from_sec1_bytes(public.as_ref()).map_err(|_| ErrorKind::Sign {
            operation: "secp256k1_verify",
            message: "invalid public key".to_owned(),
        })?;
    let signature =
        Secp256k1Signature::from_der(signature.as_ref()).map_err(|_| ErrorKind::Sign {
            operation: "secp256k1_verify",
            message: "invalid signature".to_owned(),
        })?;
    Ok(verifying.verify(data.as_ref(), &signature).is_ok())
}

fn read_file(operation: &'static str, path: &FsPath) -> Result<Vec<u8>> {
    std_fs::read(path.as_std_path()).map_err(|source| {
        Error::with_source(
            ErrorKind::Read {
                operation,
                path: path.clone(),
            },
            source,
        )
    })
}

fn random_bytes(operation: &'static str, len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0_u8; len];
    getrandom::fill(&mut bytes).map_err(|_| ErrorKind::Random { operation })?;
    Ok(bytes)
}

fn random_array<const N: usize>(operation: &'static str) -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|_| ErrorKind::Random { operation })?;
    Ok(bytes)
}

fn fixed_array<const N: usize>(
    operation: &'static str,
    name: &'static str,
    input: &[u8],
) -> Result<[u8; N]> {
    input.try_into().map_err(|_| {
        ErrorKind::Length {
            operation,
            name,
            expected: N,
            actual: input.len(),
        }
        .into()
    })
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    output
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as StdError,
        fs as test_fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn sha256_and_sha512_match_known_values() {
        assert_eq!(
            sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha512("abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
                .replace(' ', "")
        );
    }

    #[test]
    fn file_hash_reads_file() -> std::result::Result<(), Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "easy-rust-crypto-{}-{nanos}.txt",
            std::process::id()
        ));
        test_fs::write(&path, "abc")?;

        let path = path.display().to_string();
        assert_eq!(file_sha256(&path)?, sha256("abc"));
        assert_eq!(file_sha512(&path)?, sha512("abc"));
        Ok(())
    }

    #[test]
    fn common_hashes_match_known_values() {
        assert_eq!(
            hmac_sha256("key", "data"),
            "5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0"
        );
        assert_eq!(
            keccak256(""),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert_eq!(ripemd160(""), "9c1185a5c5e9fc54612808977ee8f548b2258d31");
        assert_eq!(hash160(""), "b472a266d0bd89c13706a4132ccfb16f7c3b9fcb");
    }

    #[test]
    fn secure_eq_checks_bytes() {
        assert!(secure_eq("same", "same"));
        assert!(!secure_eq("same", "diff"));
        assert!(!secure_eq("same", "same!"));
    }

    #[test]
    fn password_hash_and_verify_use_safe_password_semantics()
    -> std::result::Result<(), Box<dyn StdError>> {
        let hash = password_hash("correct horse battery staple")?;

        assert!(hash.starts_with("$argon2"));
        assert!(password_verify("correct horse battery staple", &hash)?);
        assert!(!password_verify("wrong password", &hash)?);
        Ok(())
    }

    #[test]
    fn password_verify_rejects_invalid_hash() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match password_verify("password", "not-a-password-hash") {
            Ok(value) => return Err(format!("expected password hash error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Password { operation, .. } => {
                assert_eq!(*operation, "password_verify");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        assert!(error.to_string().contains("password_verify"));
        Ok(())
    }

    #[test]
    fn aes256_encrypt_and_decrypt_roundtrip() -> std::result::Result<(), Box<dyn StdError>> {
        let key = aes256_key()?;
        let encrypted = aes256_encrypt(&key, "secret")?;
        let decrypted = aes256_decrypt(&key, encrypted)?;

        assert_eq!(decrypted, b"secret");
        Ok(())
    }

    #[test]
    fn aes256_rejects_wrong_key_length() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match aes256_encrypt([0_u8; 31], "secret") {
            Ok(value) => return Err(format!("expected key length error, got {value:?}").into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("aes256_encrypt"));
        assert!(error.to_string().contains("key length"));
        Ok(())
    }

    #[test]
    fn ed25519_sign_and_verify_roundtrip() -> std::result::Result<(), Box<dyn StdError>> {
        let pair = ed25519_keypair()?;
        let signature = ed25519_sign(pair.secret_key(), "message")?;

        assert!(ed25519_verify(pair.public_key(), "message", signature)?);
        Ok(())
    }

    #[test]
    fn secp256k1_sign_and_verify_roundtrip() -> std::result::Result<(), Box<dyn StdError>> {
        let pair = secp256k1_keypair()?;
        let signature = secp256k1_sign(pair.secret_key(), "message")?;

        assert!(secp256k1_verify(pair.public_key(), "message", signature)?);
        Ok(())
    }

    #[test]
    fn missing_file_returns_read_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match file_sha256("missing-crypto-file.txt") {
            Ok(value) => return Err(format!("expected read error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Read { path, .. } => assert_eq!(path.display(), "missing-crypto-file.txt"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
