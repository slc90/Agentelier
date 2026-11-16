use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::JoinHandle;

const STREAM_BUFFER_BYTES: usize = 512 * 1024;
const READ_CHUNK_BYTES: usize = 8192;

#[derive(Default)]
pub(crate) struct Capture {
    head: String,
    head_complete: bool,
    tail: String,
    discarded: u64,
    replaced: bool,
}

fn boundary(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

impl Capture {
    fn push(&mut self, text: &str) {
        let prefix = if self.head_complete {
            0
        } else {
            boundary(
                text,
                (STREAM_BUFFER_BYTES / 2).saturating_sub(self.head.len()),
            )
        };
        self.head_complete |= prefix < text.len();
        self.head.push_str(&text[..prefix]);
        self.tail.push_str(&text[prefix..]);
        if self.tail.len() > STREAM_BUFFER_BYTES / 2 {
            let mut remove = self.tail.len() - STREAM_BUFFER_BYTES / 2;
            while !self.tail.is_char_boundary(remove) {
                remove += 1;
            }
            self.tail.drain(..remove);
            self.discarded += remove as u64;
        }
    }

    pub(crate) fn text(&self, limit: usize) -> (String, u64, bool) {
        let full = format!("{}{}", self.head, self.tail);
        if full.len() <= limit {
            return (full, self.discarded, self.replaced);
        }
        let start = boundary(&full, limit / 2);
        let mut end = full.len().saturating_sub(limit / 2);
        while !full.is_char_boundary(end) {
            end += 1;
        }
        (
            format!("{}\n[output truncated]\n{}", &full[..start], &full[end..]),
            self.discarded + (end - start) as u64,
            self.replaced,
        )
    }
}

pub(crate) struct Reader {
    pub(crate) task: JoinHandle<io::Result<()>>,
    pub(crate) output: Arc<Mutex<Capture>>,
}

impl Reader {
    pub(crate) fn start(pipe: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let output = Arc::new(Mutex::new(Capture::default()));
        let target = output.clone();
        let task = tokio::spawn(read(pipe, target));
        Self { task, output }
    }
}

async fn read(mut pipe: impl AsyncRead + Unpin, output: Arc<Mutex<Capture>>) -> io::Result<()> {
    let mut buffer = [0; READ_CHUNK_BYTES];
    let mut pending = Vec::new();
    loop {
        let length = pipe.read(&mut buffer).await?;
        pending.extend_from_slice(&buffer[..length]);
        let mut consumed = 0;
        let mut capture = output
            .lock()
            .map_err(|_| io::Error::other("输出缓冲状态损坏"))?;
        while consumed < pending.len() {
            match std::str::from_utf8(&pending[consumed..]) {
                Ok(text) => {
                    capture.push(text);
                    consumed = pending.len();
                }
                Err(error) => {
                    let valid = consumed + error.valid_up_to();
                    capture.push(
                        std::str::from_utf8(&pending[consumed..valid]).map_err(io::Error::other)?,
                    );
                    consumed = valid;
                    if let Some(invalid) = error.error_len() {
                        capture.push("�");
                        capture.replaced = true;
                        consumed += invalid;
                    } else if length == 0 {
                        capture.push("�");
                        capture.replaced = true;
                        consumed = pending.len();
                    } else {
                        break;
                    }
                }
            }
        }
        pending.drain(..consumed);
        if length == 0 {
            return Ok(());
        }
    }
}
