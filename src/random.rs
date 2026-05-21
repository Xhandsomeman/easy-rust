//! 极简安全随机 API。
//!
//! 这个模块使用系统安全随机源，提供随机字符串、随机数字和随机选择。第一版不提供可复现种子、
//! 伪随机生成器或加权随机。

use std::{error::Error as StdError, fmt};

const ALPHANUMERIC: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const DIGITS: &[u8] = b"0123456789";

/// random 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。系统随机源不可用或范围不合法时会返回错误。
pub type Result<T> = std::result::Result<T, Error>;

/// random 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分系统随机失败或范围错误时，使用 [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind }
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
        self.kind.fmt(formatter)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match &self.kind {
            ErrorKind::Source { .. }
            | ErrorKind::InvalidRange { .. }
            | ErrorKind::InvalidUnsignedRange { .. } => None,
        }
    }
}

/// random 模块的具体错误原因。
///
/// 错误信息会包含操作名和关键上下文。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 系统安全随机源失败。
    #[error("random {operation} source failed: {message}")]
    Source {
        /// 发生错误的操作名，例如 `string`。
        operation: &'static str,
        /// 底层随机源错误信息。
        message: String,
    },

    /// 随机数字范围不合法。
    #[error("random {operation} failed: min `{min}` is greater than max `{max}`")]
    InvalidRange {
        /// 发生错误的操作名，例如 `int`。
        operation: &'static str,
        /// 范围下限。
        min: i64,
        /// 范围上限。
        max: i64,
    },

    /// 无符号随机数字范围不合法。
    #[error("random {operation} failed: min `{min}` is greater than max `{max}`")]
    InvalidUnsignedRange {
        /// 发生错误的操作名，例如 `uint`。
        operation: &'static str,
        /// 范围下限。
        min: u64,
        /// 范围上限。
        max: u64,
    },
}

/// 生成指定长度的随机字母数字字符串。
///
/// 字符集为 `a-zA-Z0-9`，随机源来自系统安全随机源。
pub fn string(length: usize) -> Result<String> {
    string_from_charset("string", length, ALPHANUMERIC)
}

/// 生成指定长度的随机数字字符串。
///
/// 字符集为 `0-9`，适合验证码等需要纯数字文本的场景。
pub fn digits(length: usize) -> Result<String> {
    string_from_charset("digits", length, DIGITS)
}

/// 生成闭区间内的随机整数。
///
/// `min` 和 `max` 都可能被返回；如果 `min > max`，返回 [`ErrorKind::InvalidRange`]。
pub fn int(min: i64, max: i64) -> Result<i64> {
    if min > max {
        return Err(ErrorKind::InvalidRange {
            operation: "int",
            min,
            max,
        }
        .into());
    }

    let size = (i128::from(max) - i128::from(min) + 1) as u128;
    let offset = sample_u128("int", size)?;
    Ok((i128::from(min) + offset as i128) as i64)
}

/// 生成闭区间内的随机无符号整数。
///
/// `min` 和 `max` 都可能被返回；如果 `min > max`，返回 [`ErrorKind::InvalidUnsignedRange`]。
pub fn uint(min: u64, max: u64) -> Result<u64> {
    if min > max {
        return Err(ErrorKind::InvalidUnsignedRange {
            operation: "uint",
            min,
            max,
        }
        .into());
    }

    let size = u128::from(max) - u128::from(min) + 1;
    let offset = sample_u128("uint", size)?;
    Ok((u128::from(min) + offset) as u64)
}

/// 生成随机延迟值。
///
/// 返回闭区间 `[min, max]` 内的随机值。`max <= min` 或系统随机源失败时返回 `min`，适合重试等待
/// 这类不希望因为随机失败而中断主流程的场景。
#[must_use]
pub fn delay(min: u64, max: u64) -> u64 {
    if max <= min {
        return min;
    }

    match uint(min, max) {
        Ok(value) => value,
        Err(_) => min,
    }
}

/// 从列表中安全随机选择一个元素。
///
/// 空列表返回 `Ok(None)`；非空列表返回元素 clone。
pub fn choice<T>(items: impl AsRef<[T]>) -> Result<Option<T>>
where
    T: Clone,
{
    let items = items.as_ref();
    if items.is_empty() {
        return Ok(None);
    }

    let index = sample_u128("choice", items.len() as u128)? as usize;
    Ok(Some(items[index].clone()))
}

fn string_from_charset(operation: &'static str, length: usize, charset: &[u8]) -> Result<String> {
    let mut output = String::with_capacity(length);

    for _ in 0..length {
        let index = sample_u128(operation, charset.len() as u128)? as usize;
        output.push(char::from(charset[index]));
    }

    Ok(output)
}

fn sample_u128(operation: &'static str, size: u128) -> Result<u128> {
    if size == 0 {
        return Ok(0);
    }

    let limit = u128::MAX - (u128::MAX % size);

    loop {
        let value = random_u128(operation)?;
        if value < limit {
            return Ok(value % size);
        }
    }
}

fn random_u128(operation: &'static str) -> Result<u128> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|source| ErrorKind::Source {
        operation,
        message: source.to_string(),
    })?;
    Ok(u128::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn string_and_digits_have_requested_length_and_charset()
    -> std::result::Result<(), Box<dyn StdError>> {
        let text = string(32)?;
        let digits = digits(12)?;

        assert_eq!(text.len(), 32);
        assert!(text.bytes().all(|byte| ALPHANUMERIC.contains(&byte)));
        assert_eq!(digits.len(), 12);
        assert!(digits.bytes().all(|byte| DIGITS.contains(&byte)));
        Ok(())
    }

    #[test]
    fn int_returns_value_in_inclusive_range() -> std::result::Result<(), Box<dyn StdError>> {
        for _ in 0..32 {
            let value = int(-2, 2)?;
            assert!((-2..=2).contains(&value));
        }
        Ok(())
    }

    #[test]
    fn uint_returns_value_in_inclusive_range() -> std::result::Result<(), Box<dyn StdError>> {
        assert_eq!(uint(7, 7)?, 7);
        for _ in 0..32 {
            let value = uint(2, 5)?;
            assert!((2..=5).contains(&value));
        }
        Ok(())
    }

    #[test]
    fn invalid_range_returns_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match int(10, 1) {
            Ok(value) => return Err(format!("expected range error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::InvalidRange {
                operation,
                min,
                max,
            } => {
                assert_eq!(*operation, "int");
                assert_eq!((*min, *max), (10, 1));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn invalid_uint_range_returns_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match uint(10, 1) {
            Ok(value) => return Err(format!("expected range error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::InvalidUnsignedRange {
                operation,
                min,
                max,
            } => {
                assert_eq!(*operation, "uint");
                assert_eq!((*min, *max), (10, 1));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn delay_returns_safe_value_without_errors() {
        assert_eq!(delay(7, 7), 7);
        assert_eq!(delay(10, 1), 10);
        for _ in 0..32 {
            let value = delay(2, 5);
            assert!((2..=5).contains(&value));
        }
    }

    #[test]
    fn choice_returns_none_for_empty_and_item_for_non_empty()
    -> std::result::Result<(), Box<dyn StdError>> {
        let empty: [i32; 0] = [];
        let value = choice([1, 2, 3])?;

        assert_eq!(choice(empty)?, None);
        assert!(matches!(value, Some(1..=3)));
        Ok(())
    }
}
