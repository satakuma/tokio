//! Tokio support for Unix FIFO files (named pipes).

use mio::unix::SourceFd;
use mio::{Registry, Token};
use std::convert::TryFrom;
use std::fs::{File, OpenOptions};
use std::io::{IoSlice, Read, Write};
use std::os::unix::prelude::OpenOptionsExt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{io, path::Path};

use std::os::unix::io::{AsRawFd, RawFd};

use crate::io::interest::Interest;
use crate::io::{AsyncRead, AsyncWrite, PollEvented, ReadBuf, Ready};

cfg_net_unix! {
    #[derive(Debug)]
    pub struct UnixNamedPipe {
        io: PollEvented<FifoFile>,
    }
}

impl UnixNamedPipe {
    /// open fifo for reading
    pub fn open_reader<P>(path: P) -> io::Result<UnixNamedPipe>
    where
        P: AsRef<Path>,
    {
        UnixNamedPipe::open_with_interest(path, Interest::READABLE)
    }

    /// open fifo for writing
    pub fn open_writer<P>(path: P) -> io::Result<UnixNamedPipe>
    where
        P: AsRef<Path>,
    {
        UnixNamedPipe::open_with_interest(path, Interest::WRITABLE)
    }

    /// use only on linux
    pub fn open<P>(path: P) -> io::Result<UnixNamedPipe>
    where
        P: AsRef<Path>,
    {
        UnixNamedPipe::open_with_interest(path, Interest::READABLE | Interest::WRITABLE)
    }

    fn open_with_interest<P>(path: P, interest: Interest) -> io::Result<UnixNamedPipe>
    where
        P: AsRef<Path>,
    {
        let file = OpenOptions::new()
            .read(interest.is_readable())
            .write(interest.is_writable())
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let fifo_file = FifoFile { file };
        let io = PollEvented::new_with_interest(fifo_file, interest)?;

        Ok(UnixNamedPipe { io })
    }

    pub fn into_file(self) -> io::Result<File> {
        self.io.into_inner().map(|ff| ff.file)
    }

    pub fn from_file(file: File) -> io::Result<UnixNamedPipe> {
        let fifo_file = FifoFile { file };
        let io = PollEvented::new(fifo_file)?;

        Ok(UnixNamedPipe { io })
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        let event = self.io.registration().readiness(interest).await?;
        Ok(event.ready)
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.ready(Interest::READABLE).await?;
        Ok(())
    }

    pub fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.registration().poll_read_ready(cx).map_ok(|_| ())
    }

    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io
            .registration()
            .try_io(Interest::READABLE, || (&*self.io).read(buf))
    }

    pub fn try_read_vectored(&self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.io
            .registration()
            .try_io(Interest::READABLE, || (&*self.io).read_vectored(bufs))
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.ready(Interest::WRITABLE).await?;
        Ok(())
    }

    pub fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.registration().poll_write_ready(cx).map_ok(|_| ())
    }

    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.io
            .registration()
            .try_io(Interest::WRITABLE, || (&*self.io).write(buf))
    }

    pub fn try_write_vectored(&self, buf: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.io
            .registration()
            .try_io(Interest::WRITABLE, || (&*self.io).write_vectored(buf))
    }

    pub fn try_io<R>(
        &self,
        interest: Interest,
        f: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        self.io.registration().try_io(interest, f) // no need for internal self.io.try_io(f) on Unix.
    }
}

impl TryFrom<File> for UnixNamedPipe {
    type Error = io::Error;

    /// equivalent to UnixNamedPipe::from_file
    fn try_from(file: File) -> io::Result<Self> {
        Self::from_file(file)
    }
}

impl AsyncRead for UnixNamedPipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_priv(cx, buf)
    }
}

impl AsyncWrite for UnixNamedPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_priv(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored_priv(cx, bufs)
    }

    // TODO
    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // No need to flush a Unix pipe.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl UnixNamedPipe {
    pub(crate) fn poll_read_priv(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // TODO Safety: `std::fs::File::read` correctly handles reads into uninitialized memory.
        unsafe { self.io.poll_read(cx, buf) }
    }

    pub(crate) fn poll_write_priv(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.io.poll_write(cx, buf)
    }

    pub(super) fn poll_write_vectored_priv(
        &self,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.io.poll_write_vectored(cx, bufs)
    }
}

impl AsRawFd for UnixNamedPipe {
    fn as_raw_fd(&self) -> RawFd {
        self.io.as_raw_fd()
    }
}

#[derive(Debug)]
struct FifoFile {
    file: File,
}

impl Read for &FifoFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.file).read(buf)
    }
}

impl Write for &FifoFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.file).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&self.file).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        // No need to flush a unix pipe.
        Ok(())
    }
}

impl mio::event::Source for FifoFile {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        SourceFd(&self.file.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        SourceFd(&self.file.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.file.as_raw_fd()).deregister(registry)
    }
}

impl AsRawFd for FifoFile {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}
