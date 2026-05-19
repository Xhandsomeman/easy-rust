//! 极简编码 API。
//!
//! 这个模块提供 Base64、Base58、十六进制和常用整数大小端编码。编码函数返回字符串或字节数组，
//! 解码函数失败时返回带操作名和输入预览的错误。

use std::{error::Error as StdError, fmt};

use base64_crate::{Engine as _, engine::general_purpose::STANDARD};

const INPUT_PREVIEW_CHARS: usize = 160;

/// codec 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let bytes = codec::hex_decode(text)?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// codec 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分 Base64 或十六进制解码错误时，
/// 使用 [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn StdError + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + 'static) -> Self {
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
        self.source.as_deref()
    }
}

/// codec 模块的具体错误原因。
///
/// 错误信息会包含操作名和输入预览，方便定位哪段文本无法解码。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// Base64 解码失败。
    #[error("codec {operation} `{input}` failed")]
    Base64Decode {
        /// 发生错误的操作名，例如 `base64_decode`。
        operation: &'static str,
        /// 输入内容预览。
        input: String,
    },

    /// Base58 解码失败。
    #[error("codec {operation} `{input}` failed")]
    Base58Decode {
        /// 发生错误的操作名，例如 `base58_decode`。
        operation: &'static str,
        /// 输入内容预览。
        input: String,
    },

    /// 十六进制解码失败。
    #[error("codec {operation} `{input}` failed: {message}")]
    HexDecode {
        /// 发生错误的操作名，例如 `hex_decode`。
        operation: &'static str,
        /// 输入内容预览。
        input: String,
        /// 面向人的十六进制错误说明。
        message: String,
    },

    /// 固定长度字节转换失败。
    #[error("codec {operation} failed: input length must be {expected}, got {actual}")]
    ByteLength {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 期望字节长度。
        expected: usize,
        /// 实际字节长度。
        actual: usize,
    },
}

/// Base64 编码。
///
/// 输入可以是 `&str`、`String`、`&[u8]` 或 `Vec<u8>`。
#[must_use]
pub fn base64_encode(input: impl AsRef<[u8]>) -> String {
    STANDARD.encode(input.as_ref())
}

/// Base64 解码。
///
/// 输入非法时返回 [`ErrorKind::Base64Decode`]，错误包含输入预览。
pub fn base64_decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let input = input.as_ref();
    STANDARD.decode(input).map_err(|source| {
        Error::with_source(
            ErrorKind::Base64Decode {
                operation: "base64_decode",
                input: bytes_preview(input),
            },
            source,
        )
    })
}

/// Base58 编码。
///
/// 常用于区块链地址、短 token 和不易混淆的文本传输。
#[must_use]
pub fn base58_encode(input: impl AsRef<[u8]>) -> String {
    bs58::encode(input.as_ref()).into_string()
}

/// Base58 解码。
///
/// 输入非法时返回 [`ErrorKind::Base58Decode`]，错误包含输入预览。
pub fn base58_decode(input: impl AsRef<str>) -> Result<Vec<u8>> {
    let input = input.as_ref();
    bs58::decode(input).into_vec().map_err(|source| {
        Error::with_source(
            ErrorKind::Base58Decode {
                operation: "base58_decode",
                input: input_preview(input),
            },
            source,
        )
    })
}

/// 十六进制编码。
///
/// 输出使用小写字母，输入可以是 `&str`、`String`、`&[u8]` 或 `Vec<u8>`。
#[must_use]
pub fn hex_encode(input: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let input = input.as_ref();
    let mut output = String::with_capacity(input.len() * 2);

    for byte in input {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    output
}

/// 十六进制解码。
///
/// 输入长度必须为偶数，且只能包含 `0-9a-fA-F`。
pub fn hex_decode(input: impl AsRef<str>) -> Result<Vec<u8>> {
    let input = input.as_ref();
    let bytes = input.as_bytes();

    if bytes.len() % 2 != 0 {
        return Err(hex_error(input, "hex length must be even"));
    }

    let mut output = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let high = hex_value(bytes[index]);
        let low = hex_value(bytes[index + 1]);
        match (high, low) {
            (Some(high), Some(low)) => output.push((high << 4) | low),
            _ => return Err(hex_error(input, "invalid hex character")),
        }
        index += 2;
    }

    Ok(output)
}

/// 把 `u16` 转成大端字节。
#[must_use]
pub fn u16_to_be_bytes(value: u16) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// 从大端字节读取 `u16`。
pub fn u16_from_be_bytes(bytes: impl AsRef<[u8]>) -> Result<u16> {
    Ok(u16::from_be_bytes(fixed_bytes(
        "u16_from_be_bytes",
        bytes.as_ref(),
    )?))
}

/// 把 `u16` 转成小端字节。
#[must_use]
pub fn u16_to_le_bytes(value: u16) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

/// 从小端字节读取 `u16`。
pub fn u16_from_le_bytes(bytes: impl AsRef<[u8]>) -> Result<u16> {
    Ok(u16::from_le_bytes(fixed_bytes(
        "u16_from_le_bytes",
        bytes.as_ref(),
    )?))
}

/// 把 `u32` 转成大端字节。
#[must_use]
pub fn u32_to_be_bytes(value: u32) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// 从大端字节读取 `u32`。
pub fn u32_from_be_bytes(bytes: impl AsRef<[u8]>) -> Result<u32> {
    Ok(u32::from_be_bytes(fixed_bytes(
        "u32_from_be_bytes",
        bytes.as_ref(),
    )?))
}

/// 把 `u32` 转成小端字节。
#[must_use]
pub fn u32_to_le_bytes(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

/// 从小端字节读取 `u32`。
pub fn u32_from_le_bytes(bytes: impl AsRef<[u8]>) -> Result<u32> {
    Ok(u32::from_le_bytes(fixed_bytes(
        "u32_from_le_bytes",
        bytes.as_ref(),
    )?))
}

/// 把 `u64` 转成大端字节。
#[must_use]
pub fn u64_to_be_bytes(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// 从大端字节读取 `u64`。
pub fn u64_from_be_bytes(bytes: impl AsRef<[u8]>) -> Result<u64> {
    Ok(u64::from_be_bytes(fixed_bytes(
        "u64_from_be_bytes",
        bytes.as_ref(),
    )?))
}

/// 把 `u64` 转成小端字节。
#[must_use]
pub fn u64_to_le_bytes(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

/// 从小端字节读取 `u64`。
pub fn u64_from_le_bytes(bytes: impl AsRef<[u8]>) -> Result<u64> {
    Ok(u64::from_le_bytes(fixed_bytes(
        "u64_from_le_bytes",
        bytes.as_ref(),
    )?))
}

fn hex_error(input: &str, message: &str) -> Error {
    ErrorKind::HexDecode {
        operation: "hex_decode",
        input: input_preview(input),
        message: message.to_owned(),
    }
    .into()
}

fn fixed_bytes<const N: usize>(operation: &'static str, bytes: &[u8]) -> Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        ErrorKind::ByteLength {
            operation,
            expected: N,
            actual: bytes.len(),
        }
        .into()
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn bytes_preview(input: &[u8]) -> String {
    input_preview(&String::from_utf8_lossy(input))
}

fn input_preview(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();

    for _ in 0..INPUT_PREVIEW_CHARS {
        let Some(ch) = chars.next() else {
            return output;
        };
        output.push(ch);
    }

    if chars.next().is_some() {
        output.push_str("...");
    }

    output
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn base64_roundtrips_bytes() -> std::result::Result<(), Box<dyn StdError>> {
        let encoded = base64_encode("hello");
        let decoded = base64_decode(&encoded)?;

        assert_eq!(encoded, "aGVsbG8=");
        assert_eq!(decoded, b"hello");
        Ok(())
    }

    #[test]
    fn hex_roundtrips_bytes() -> std::result::Result<(), Box<dyn StdError>> {
        let encoded = hex_encode("hello");
        let decoded = hex_decode(&encoded)?;

        assert_eq!(encoded, "68656c6c6f");
        assert_eq!(decoded, b"hello");
        assert_eq!(hex_decode("68656C6C6F")?, b"hello");
        Ok(())
    }

    #[test]
    fn base58_roundtrips_bytes() -> std::result::Result<(), Box<dyn StdError>> {
        let encoded = base58_encode("hello");
        let decoded = base58_decode(&encoded)?;

        assert_eq!(decoded, b"hello");
        Ok(())
    }

    #[test]
    fn integer_bytes_roundtrip() -> std::result::Result<(), Box<dyn StdError>> {
        assert_eq!(u16_from_be_bytes(u16_to_be_bytes(0x1234))?, 0x1234);
        assert_eq!(u16_from_le_bytes(u16_to_le_bytes(0x1234))?, 0x1234);
        assert_eq!(
            u32_from_be_bytes(u32_to_be_bytes(0x1234_5678))?,
            0x1234_5678
        );
        assert_eq!(
            u32_from_le_bytes(u32_to_le_bytes(0x1234_5678))?,
            0x1234_5678
        );
        assert_eq!(
            u64_from_be_bytes(u64_to_be_bytes(0x1234_5678_90ab_cdef))?,
            0x1234_5678_90ab_cdef
        );
        assert_eq!(
            u64_from_le_bytes(u64_to_le_bytes(0x1234_5678_90ab_cdef))?,
            0x1234_5678_90ab_cdef
        );
        Ok(())
    }

    #[test]
    fn invalid_base64_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match base64_decode("not base64!") {
            Ok(value) => return Err(format!("expected base64 error, got {value:?}").into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("base64_decode"));
        assert!(error.to_string().contains("not base64"));
        Ok(())
    }

    #[test]
    fn invalid_hex_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let odd = match hex_decode("abc") {
            Ok(value) => return Err(format!("expected odd hex error, got {value:?}").into()),
            Err(error) => error,
        };
        let invalid = match hex_decode("zz") {
            Ok(value) => return Err(format!("expected invalid hex error, got {value:?}").into()),
            Err(error) => error,
        };

        assert!(odd.to_string().contains("hex_decode"));
        assert!(odd.to_string().contains("abc"));
        assert!(invalid.to_string().contains("zz"));
        Ok(())
    }

    #[test]
    fn invalid_base58_and_integer_length_return_context_errors()
    -> std::result::Result<(), Box<dyn StdError>> {
        let base58 = match base58_decode("0") {
            Ok(value) => return Err(format!("expected base58 error, got {value:?}").into()),
            Err(error) => error,
        };
        let int = match u32_from_be_bytes([1_u8, 2]) {
            Ok(value) => return Err(format!("expected integer length error, got {value}").into()),
            Err(error) => error,
        };

        assert!(base58.to_string().contains("base58_decode"));
        assert!(int.to_string().contains("u32_from_be_bytes"));
        Ok(())
    }
}
