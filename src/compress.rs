//! 极简压缩和压缩包 API。
//!
//! 这个模块统一提供 gzip、zip 和 tar 的压缩、解压、打包和解包能力。输出文件会自动创建父目录，
//! 输出目录会自动创建；不提供压缩等级、加密、分卷、过滤器或 streaming API。

use std::{
    error::Error as StdError,
    fmt,
    fs::{self as std_fs, File},
    io::{self, Read, Write},
    path::{Component, Path as StdPath, PathBuf},
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use zip_crate::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::fs::Path as FsPath;

const INPUT_PREVIEW_BYTES: usize = 80;

/// compress 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `compress::gzip_file("app.log", "app.log.gz")?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// compress 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分读写、gzip、zip 或 tar 错误时，使用
/// [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: Box<ErrorKind>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self {
            kind: Box::new(kind),
            source: None,
        }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + Send + Sync + 'static) -> Self {
        Self {
            kind: Box::new(kind),
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

/// compress 模块的具体错误原因。
///
/// 错误信息会包含操作名和路径或输入预览，方便定位压缩、解压、打包或解包失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 读取文件或目录失败。
    #[error("compress {operation} read `{path}` failed")]
    Read {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 写入文件失败。
    #[error("compress {operation} write `{path}` failed")]
    Write {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 创建目录失败。
    #[error("compress {operation} make_dir `{path}` failed")]
    MakeDir {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 发生错误的目录路径。
        path: FsPath,
    },

    /// gzip 处理失败。
    #[error("compress {operation} `{input}` failed")]
    Gzip {
        /// 发生错误的操作名，例如 `gzip` 或 `gunzip`。
        operation: &'static str,
        /// 输入内容或路径的短预览。
        input: String,
    },

    /// zip 处理失败。
    #[error("compress {operation} `{path}` failed")]
    Zip {
        /// 发生错误的操作名，例如 `zip` 或 `unzip`。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },

    /// tar 处理失败。
    #[error("compress {operation} `{path}` failed")]
    Tar {
        /// 发生错误的操作名，例如 `tar` 或 `untar`。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 压缩包路径形状不合法。
    #[error("compress {operation} `{path}` failed: {message}")]
    Shape {
        /// 发生错误的操作名。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
        /// 面向人的形状错误说明。
        message: String,
    },
}

/// gzip 压缩字节内容。
///
/// 返回 gzip 格式的字节数组。适合压缩内存中的文本、JSON 或二进制内容。
pub fn gzip(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let input = input.as_ref();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|source| gzip_error("gzip", bytes_preview(input), source))?;
    encoder
        .finish()
        .map_err(|source| gzip_error("gzip", bytes_preview(input), source))
}

/// 解压 gzip 字节内容。
///
/// 输入不是合法 gzip 时返回 [`ErrorKind::Gzip`]，错误包含输入预览。
pub fn gunzip(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let input = input.as_ref();
    let mut decoder = GzDecoder::new(input);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|source| gzip_error("gunzip", bytes_preview(input), source))?;
    Ok(output)
}

/// 把文件压缩成 gzip 文件。
///
/// 输出文件父目录会自动创建。输入文件会完整读入内存，适合常见脚本和后端工具场景。
pub fn gzip_file(input: impl Into<FsPath>, output: impl Into<FsPath>) -> Result<()> {
    let input = input.into();
    let output = output.into();
    let bytes = std_fs::read(input.as_std_path())
        .map_err(|source| read_error("gzip_file", &input, source))?;
    let compressed = gzip(&bytes)?;
    create_parent_dirs("gzip_file", &output)?;
    std_fs::write(output.as_std_path(), compressed)
        .map_err(|source| write_error("gzip_file", &output, source))
}

/// 解压 gzip 文件。
///
/// 输出文件父目录会自动创建。输入不是合法 gzip 时返回 [`ErrorKind::Gzip`]。
pub fn gunzip_file(input: impl Into<FsPath>, output: impl Into<FsPath>) -> Result<()> {
    let input = input.into();
    let output = output.into();
    let bytes = std_fs::read(input.as_std_path())
        .map_err(|source| read_error("gunzip_file", &input, source))?;
    let decompressed = gunzip(&bytes).map_err(|error| match *error.kind {
        ErrorKind::Gzip { .. } => ErrorKind::Gzip {
            operation: "gunzip_file",
            input: input.display(),
        }
        .into(),
        other => Error::new(other),
    })?;
    create_parent_dirs("gunzip_file", &output)?;
    std_fs::write(output.as_std_path(), decompressed)
        .map_err(|source| write_error("gunzip_file", &output, source))
}

/// 把文件或目录打包成 zip。
///
/// 输出文件的父目录会自动创建。目录会以目录名作为 zip 根路径，文件会以文件名作为 zip 根路径。
pub fn zip(src: impl Into<FsPath>, dest: impl Into<FsPath>) -> Result<()> {
    let src = src.into();
    let dest = dest.into();
    create_parent_dirs("zip", &dest)?;
    let file =
        File::create(dest.as_std_path()).map_err(|source| write_error("zip", &dest, source))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let root = root_name(&src, "zip")?;

    add_zip_path(&mut writer, src.as_std_path(), &root, options)?;
    writer
        .finish()
        .map_err(|source| zip_error("zip", &dest, source))?;
    Ok(())
}

/// 解压 zip 文件到目录。
///
/// 输出目录会自动创建。zip 内部路径必须是相对安全路径，非法路径会返回 [`ErrorKind::Shape`]。
pub fn unzip(src: impl Into<FsPath>, dest: impl Into<FsPath>) -> Result<()> {
    let src = src.into();
    let dest = dest.into();
    create_dir("unzip", &dest)?;
    let file = File::open(src.as_std_path()).map_err(|source| read_error("unzip", &src, source))?;
    let mut archive = ZipArchive::new(file).map_err(|source| zip_error("unzip", &src, source))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|source| zip_error("unzip", &src, source))?;
        let enclosed = entry.enclosed_name().ok_or_else(|| ErrorKind::Shape {
            operation: "unzip",
            path: src.clone(),
            message: "unsafe zip entry path".to_owned(),
        })?;
        let output = dest.as_std_path().join(enclosed);

        if entry.is_dir() {
            std_fs::create_dir_all(&output).map_err(|source| {
                make_dir_error("unzip", FsPath::from_std_path(&output), source)
            })?;
            continue;
        }

        if let Some(parent) = output.parent() {
            std_fs::create_dir_all(parent)
                .map_err(|source| make_dir_error("unzip", FsPath::from_std_path(parent), source))?;
        }

        let mut file = File::create(&output)
            .map_err(|source| write_error("unzip", &FsPath::from_std_path(&output), source))?;
        io::copy(&mut entry, &mut file)
            .map_err(|source| write_error("unzip", &FsPath::from_std_path(&output), source))?;
    }

    Ok(())
}

/// 把文件或目录打包成 tar。
///
/// 输出文件的父目录会自动创建。第一版只处理普通 tar，不做 tar.gz/xz/zstd。
pub fn tar(src: impl Into<FsPath>, dest: impl Into<FsPath>) -> Result<()> {
    let src = src.into();
    let dest = dest.into();
    create_parent_dirs("tar", &dest)?;
    let file =
        File::create(dest.as_std_path()).map_err(|source| write_error("tar", &dest, source))?;
    let mut builder = tar_crate::Builder::new(file);
    let root = root_name(&src, "tar")?;

    if src.as_std_path().is_dir() {
        builder
            .append_dir_all(&root, src.as_std_path())
            .map_err(|source| tar_error("tar", &src, source))?;
    } else {
        builder
            .append_path_with_name(src.as_std_path(), &root)
            .map_err(|source| tar_error("tar", &src, source))?;
    }

    builder
        .finish()
        .map_err(|source| tar_error("tar", &dest, source))?;
    Ok(())
}

/// 解包 tar 文件到目录。
///
/// 输出目录会自动创建。tar 内部路径必须是相对安全路径，非法路径会返回 [`ErrorKind::Shape`]。
pub fn untar(src: impl Into<FsPath>, dest: impl Into<FsPath>) -> Result<()> {
    let src = src.into();
    let dest = dest.into();
    create_dir("untar", &dest)?;
    let file = File::open(src.as_std_path()).map_err(|source| read_error("untar", &src, source))?;
    let mut archive = tar_crate::Archive::new(file);
    let entries = archive
        .entries()
        .map_err(|source| tar_error("untar", &src, source))?;

    for entry in entries {
        let mut entry = entry.map_err(|source| tar_error("untar", &src, source))?;
        let entry_path = entry
            .path()
            .map_err(|source| tar_error("untar", &src, source))?;
        validate_archive_path(entry_path.as_ref(), "untar", &src)?;
        entry
            .unpack_in(dest.as_std_path())
            .map_err(|source| tar_error("untar", &src, source))?;
    }
    Ok(())
}

fn add_zip_path(
    writer: &mut ZipWriter<File>,
    path: &StdPath,
    archive_path: &StdPath,
    options: SimpleFileOptions,
) -> Result<()> {
    let metadata = std_fs::metadata(path)
        .map_err(|source| read_error("zip", &FsPath::from_std_path(path), source))?;

    if metadata.is_dir() {
        let name = zip_name(archive_path);
        writer
            .add_directory(&name, options)
            .map_err(|source| zip_error("zip", &FsPath::from_std_path(path), source))?;

        for entry in std_fs::read_dir(path)
            .map_err(|source| read_error("zip", &FsPath::from_std_path(path), source))?
        {
            let entry =
                entry.map_err(|source| read_error("zip", &FsPath::from_std_path(path), source))?;
            add_zip_path(
                writer,
                &entry.path(),
                &archive_path.join(entry.file_name()),
                options,
            )?;
        }
    } else {
        let name = zip_name(archive_path);
        writer
            .start_file(&name, options)
            .map_err(|source| zip_error("zip", &FsPath::from_std_path(path), source))?;
        let mut file = File::open(path)
            .map_err(|source| read_error("zip", &FsPath::from_std_path(path), source))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|source| read_error("zip", &FsPath::from_std_path(path), source))?;
        writer
            .write_all(&buffer)
            .map_err(|source| write_error("zip", &FsPath::from_std_path(path), source))?;
    }

    Ok(())
}

fn root_name(path: &FsPath, operation: &'static str) -> Result<PathBuf> {
    path.as_std_path()
        .file_name()
        .map(PathBuf::from)
        .ok_or_else(|| {
            ErrorKind::Shape {
                operation,
                path: path.clone(),
                message: "path has no file name".to_owned(),
            }
            .into()
        })
}

fn zip_name(path: &StdPath) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn validate_archive_path(path: &StdPath, operation: &'static str, archive: &FsPath) -> Result<()> {
    let unsafe_path = path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        });

    if unsafe_path {
        return Err(ErrorKind::Shape {
            operation,
            path: archive.clone(),
            message: format!("unsafe archive entry path `{}`", path.display()),
        }
        .into());
    }

    Ok(())
}

fn create_parent_dirs(operation: &'static str, path: &FsPath) -> Result<()> {
    if let Some(parent) = path.as_std_path().parent()
        && !parent.as_os_str().is_empty()
    {
        std_fs::create_dir_all(parent)
            .map_err(|source| make_dir_error(operation, FsPath::from_std_path(parent), source))?;
    }

    Ok(())
}

fn create_dir(operation: &'static str, path: &FsPath) -> Result<()> {
    std_fs::create_dir_all(path.as_std_path())
        .map_err(|source| make_dir_error(operation, path.clone(), source))
}

fn read_error(operation: &'static str, path: &FsPath, source: io::Error) -> Error {
    Error::with_source(
        ErrorKind::Read {
            operation,
            path: path.clone(),
        },
        source,
    )
}

fn write_error(operation: &'static str, path: &FsPath, source: io::Error) -> Error {
    Error::with_source(
        ErrorKind::Write {
            operation,
            path: path.clone(),
        },
        source,
    )
}

fn make_dir_error(operation: &'static str, path: FsPath, source: io::Error) -> Error {
    Error::with_source(ErrorKind::MakeDir { operation, path }, source)
}

fn gzip_error(operation: &'static str, input: String, source: io::Error) -> Error {
    Error::with_source(ErrorKind::Gzip { operation, input }, source)
}

fn zip_error(operation: &'static str, path: &FsPath, source: zip_crate::result::ZipError) -> Error {
    Error::with_source(
        ErrorKind::Zip {
            operation,
            path: path.clone(),
        },
        source,
    )
}

fn tar_error(operation: &'static str, path: &FsPath, source: io::Error) -> Error {
    Error::with_source(
        ErrorKind::Tar {
            operation,
            path: path.clone(),
        },
        source,
    )
}

fn bytes_preview(input: &[u8]) -> String {
    let mut output = String::new();
    for byte in input.iter().take(INPUT_PREVIEW_BYTES) {
        output.push_str(&format!("{byte:02x}"));
    }
    if input.len() > INPUT_PREVIEW_BYTES {
        output.push_str("...");
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

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "easy-rust-compress-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        test_fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn path_text(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn gzip_and_gunzip_roundtrip_bytes() -> std::result::Result<(), Box<dyn StdError>> {
        let compressed = gzip("hello gzip")?;
        let decompressed = gunzip(compressed)?;

        assert_eq!(decompressed, b"hello gzip");
        Ok(())
    }

    #[test]
    fn gzip_file_and_gunzip_file_roundtrip() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("gzip-file")?;
        let input = root.join("input.txt");
        let compressed = root.join("out/input.txt.gz");
        let output = root.join("out/input.txt");
        test_fs::write(&input, "hello file")?;

        gzip_file(path_text(&input), path_text(&compressed))?;
        gunzip_file(path_text(&compressed), path_text(&output))?;

        assert_eq!(test_fs::read_to_string(output)?, "hello file");
        Ok(())
    }

    #[test]
    fn invalid_gzip_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match gunzip("not gzip") {
            Ok(value) => return Err(format!("expected gzip error, got {value:?}").into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("gunzip"));
        Ok(())
    }

    #[test]
    fn zip_and_unzip_roundtrip_directory() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("zip")?;
        let src = root.join("src");
        let archive = root.join("out/archive.zip");
        let dest = root.join("dest");
        test_fs::create_dir_all(src.join("nested"))?;
        test_fs::write(src.join("nested/file.txt"), "hello")?;

        zip(path_text(&src), path_text(&archive))?;
        unzip(path_text(&archive), path_text(&dest))?;

        assert_eq!(
            test_fs::read_to_string(dest.join("src/nested/file.txt"))?,
            "hello"
        );
        Ok(())
    }

    #[test]
    fn tar_and_untar_roundtrip_directory() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("tar")?;
        let src = root.join("src");
        let archive = root.join("out/archive.tar");
        let dest = root.join("dest");
        test_fs::create_dir_all(src.join("nested"))?;
        test_fs::write(src.join("nested/file.txt"), "hello")?;

        tar(path_text(&src), path_text(&archive))?;
        untar(path_text(&archive), path_text(&dest))?;

        assert_eq!(
            test_fs::read_to_string(dest.join("src/nested/file.txt"))?,
            "hello"
        );
        Ok(())
    }

    #[test]
    fn untar_rejects_unsafe_entry_path() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("unsafe-tar")?;
        let archive = root.join("bad.tar");
        let dest = root.join("dest");
        let file = test_fs::File::create(&archive)?;
        let mut builder = tar_crate::Builder::new(file);
        let mut header = tar_crate::Header::new_gnu();
        let body = b"escape";
        header.set_size(body.len() as u64);
        header.set_entry_type(tar_crate::EntryType::Regular);
        header.set_mode(0o644);
        header.as_mut_bytes()[0..13].copy_from_slice(b"../escape.txt");
        header.set_cksum();
        builder.append(&header, &body[..])?;
        builder.finish()?;

        let error = match untar(path_text(&archive), path_text(&dest)) {
            Ok(()) => return Err("expected unsafe path error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Shape {
                operation, message, ..
            } => {
                assert_eq!(*operation, "untar");
                assert!(message.contains("unsafe archive entry path"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        Ok(())
    }

    #[test]
    fn missing_source_returns_read_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("missing")?;
        let error = match zip(
            path_text(&root.join("missing")),
            path_text(&root.join("out.zip")),
        ) {
            Ok(()) => return Err("expected read error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Read { operation, .. } => assert_eq!(*operation, "zip"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
