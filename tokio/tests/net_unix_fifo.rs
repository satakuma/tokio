#![cfg(feature = "full")]
#![cfg(unix)]

use std::ffi::CString;
use std::path::{Path, PathBuf};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixNamedPipe;

use tokio_test::task;
use tokio_test::{assert_pending, assert_ready_ok};

struct TempFifo {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl TempFifo {
    fn new(name: &str) -> io::Result<TempFifo> {
        let dir = tempfile::Builder::new()
            .prefix("tokio-fifo-tests")
            .tempdir()?;
        let path = dir.path().join(name);
        let cpath = CString::new(path.as_path().to_str().unwrap()).unwrap();
        let result = unsafe { libc::mkfifo(cpath.as_ptr(), 0o644) };
        if result == 0 {
            Ok(TempFifo { path, _dir: dir })
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "mkfifo"))
        }
    }
}

impl AsRef<Path> for TempFifo {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

#[tokio::test]
async fn simple_send() -> io::Result<()> {
    let fifo = TempFifo::new("simple_send")?;
    const DATA: &[u8] = b"this is some data to write to the fifo";

    let mut reader = UnixNamedPipe::open_reader(&fifo)?;
    let mut read_fut = task::spawn(async move {
        let mut buf = vec![0; DATA.len()];
        reader.read_exact(&mut buf).await?;
        Ok::<_, io::Error>(buf)
    });
    assert_pending!(read_fut.poll());

    let mut writer = UnixNamedPipe::open_writer(&fifo)?;
    writer.write_all(DATA).await?;

    // Let the IO driver poll events for the reader, works thanks to #5223.
    tokio::task::yield_now().await;

    let read_data = assert_ready_ok!(read_fut.poll());
    assert_eq!(&read_data, DATA);

    Ok(())
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn simple_send_write_first() -> io::Result<()> {
    use tokio_test::assert_err;

    let fifo = TempFifo::new("simple_send_write_first")?;
    const DATA: &[u8] = b"this is some data to write to the fifo";

    assert_err!(UnixNamedPipe::open_writer(&fifo));

    let mut writer = UnixNamedPipe::open(&fifo)?;
    writer.write_all(DATA).await?;

    let mut reader = UnixNamedPipe::open_reader(&fifo)?;
    let mut read_data = vec![0; DATA.len()];
    reader.read_exact(&mut read_data).await?;
    assert_eq!(&read_data, DATA);

    Ok(())
}
