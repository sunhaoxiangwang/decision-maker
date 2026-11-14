//! Bounded localhost SSE fan-out. Capture only enqueues; socket writes run per client.
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_CLIENTS: usize = 8;
const CLIENT_QUEUE: usize = 16;
const REPLAY_BYTES: usize = 1024 * 1024;
const REPLAY_EVENTS: usize = 128;
const IO_DEADLINE: Duration = Duration::from_millis(500);

struct Client {
    id: usize,
    sender: SyncSender<Arc<str>>,
    socket: TcpStream,
}

#[derive(Default)]
struct State {
    clients: Vec<Client>,
    sequence: u64,
    replay: VecDeque<(u64, Arc<str>)>,
    replay_bytes: usize,
    mode: Option<Arc<str>>,
    memory: Option<Arc<str>>,
    status: Option<Arc<str>>,
}

#[derive(Default)]
pub(crate) struct Hub {
    state: Mutex<State>,
    connections: Arc<AtomicUsize>,
    next_client: AtomicUsize,
}

impl Hub {
    pub fn serve(self: Arc<Self>, port: u16, origin: String) -> io::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if self
                    .connections
                    .fetch_update(SeqCst, SeqCst, |n| (n < MAX_CLIENTS).then_some(n + 1))
                    .is_err()
                {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let hub = self.clone();
                let origin = origin.clone();
                std::thread::spawn(move || {
                    let _ = hub.client(stream, port, &origin);
                    hub.connections.fetch_sub(1, SeqCst);
                });
            }
        });
        Ok(())
    }

    fn client(&self, mut stream: TcpStream, port: u16, origin: &str) -> io::Result<()> {
        stream.set_write_timeout(Some(IO_DEADLINE))?;
        let request = read_head(&mut stream)?;
        if !request.starts_with("GET /events HTTP/1.1\r\n") || !allowed(&request, port, origin) {
            return write_frame(
                &mut stream,
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
            );
        }
        let (sender, receiver) = sync_channel(CLIENT_QUEUE);
        let cursor = header(&request, "last-event-id").and_then(|n| n.parse::<u64>().ok());
        let id = self.next_client.fetch_add(1, SeqCst);
        let initial = {
            let mut state = self.state.lock().unwrap();
            let mut frames = Vec::new();
            if let Some(cursor) = cursor {
                let first = state
                    .replay
                    .front()
                    .map_or(state.sequence + 1, |(id, _)| *id);
                if cursor <= state.sequence && cursor.saturating_add(1) >= first {
                    frames.extend(
                        state
                            .replay
                            .iter()
                            .filter(|(id, _)| *id > cursor)
                            .map(|(_, f)| f.clone()),
                    );
                } else {
                    frames.push(Arc::from("data: {\"type\":\"gap\",\"message\":\"Some bridge events expired; history may be incomplete.\"}\n\n"));
                    frames.extend(
                        [
                            state.mode.clone(),
                            state.memory.clone(),
                            state.status.clone(),
                        ]
                        .into_iter()
                        .flatten(),
                    );
                }
            } else {
                frames.extend(
                    [
                        state.mode.clone(),
                        state.memory.clone(),
                        state.status.clone(),
                    ]
                    .into_iter()
                    .flatten(),
                );
            }
            state.clients.push(Client {
                id,
                sender,
                socket: stream.try_clone()?,
            });
            frames
        };
        let _registration = Registration { hub: self, id };
        write_frame(&mut stream, format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nConnection: keep-alive\r\n\r\n").as_bytes())?;
        let catchup_deadline = Instant::now() + Duration::from_secs(2);
        for frame in initial {
            if Instant::now() > catchup_deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            write_frame(&mut stream, frame.as_bytes())?;
        }
        stream.set_read_timeout(Some(Duration::from_millis(1)))?;
        let mut heartbeat = Instant::now();
        loop {
            match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(frame) => write_frame(&mut stream, frame.as_bytes())?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    match stream.peek(&mut [0]) {
                        Ok(0) => return Ok(()),
                        Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) =>
                        {
                            ()
                        }
                        Err(e) => return Err(e),
                    }
                    if heartbeat.elapsed() >= Duration::from_secs(10) {
                        write_frame(&mut stream, b": keepalive\n\n")?;
                        heartbeat = Instant::now();
                    }
                }
                Err(_) => return Ok(()),
            }
        }
    }

    pub fn send(&self, json: &str) -> bool {
        if json.len() > MAX_EVENT_BYTES {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        state.sequence += 1;
        let frame: Arc<str> = Arc::from(format!("id: {}\ndata: {json}\n\n", state.sequence));
        if json.starts_with("{\"type\":\"mode\"") {
            state.mode = Some(Arc::from(format!("data: {json}\n\n")));
        } else if json.starts_with("{\"type\":\"gesture\"") {
            state.memory = Some(Arc::from(format!("data: {json}\n\n")));
        } else if json.starts_with("{\"type\":\"status\"") {
            state.status = Some(Arc::from(format!("data: {json}\n\n")));
        }
        let id = state.sequence;
        state.replay_bytes += frame.len();
        state.replay.push_back((id, frame.clone()));
        while state.replay.len() > REPLAY_EVENTS || state.replay_bytes > REPLAY_BYTES {
            state.replay_bytes -= state.replay.pop_front().unwrap().1.len();
        }
        state.clients.retain(|client| {
            if client.sender.try_send(frame.clone()).is_ok() {
                return true;
            }
            // Wakes a writer blocked in write_all as well as dropping its bounded queue.
            let _ = client.socket.shutdown(Shutdown::Both);
            false
        });
        true
    }
}

struct Registration<'a> {
    hub: &'a Hub,
    id: usize,
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.hub
            .state
            .lock()
            .unwrap()
            .clients
            .retain(|c| c.id != self.id);
    }
}

fn write_frame(stream: &mut TcpStream, mut bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + IO_DEADLINE;
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(io::ErrorKind::TimedOut)?;
        stream.set_write_timeout(Some(remaining))?;
        let n = stream.write(bytes)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[n..];
    }
    Ok(())
}

pub(crate) fn read_head(stream: &mut TcpStream) -> io::Result<String> {
    let deadline = Instant::now() + IO_DEADLINE;
    let mut head = Vec::new();
    // Read only the headers: API handlers may need the body left in the socket.
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() == 4096 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(io::ErrorKind::TimedOut)?;
        stream.set_read_timeout(Some(remaining))?;
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        head.push(byte[0]);
    }
    String::from_utf8(head).map_err(|_| io::ErrorKind::InvalidData.into())
}

pub(crate) fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

pub(crate) fn allowed(request: &str, port: u16, origin: &str) -> bool {
    let host_ok = header(request, "host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    host_ok && header(request, "origin").map_or(true, |o| o == origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_subscriber_queue_is_evicted_without_blocking_other_subscribers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        let (slow, _unread) = sync_channel(1);
        slow.try_send(Arc::from("already full")).unwrap();
        let (fast, read) = sync_channel(1);
        let hub = Hub::default();
        hub.state.lock().unwrap().clients = vec![
            Client {
                id: 0,
                sender: slow,
                socket: socket.try_clone().unwrap(),
            },
            Client {
                id: 1,
                sender: fast,
                socket,
            },
        ];
        let start = Instant::now();
        assert!(hub.send("{\"type\":\"test\"}"));
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(read.try_recv().unwrap().contains("test"));
        assert_eq!(hub.state.lock().unwrap().clients.len(), 1);
        drop(peer);
    }

    #[test]
    fn replay_and_event_bytes_are_bounded() {
        let hub = Hub::default();
        for _ in 0..1000 {
            assert!(hub.send(&"x".repeat(32 * 1024)));
        }
        let state = hub.state.lock().unwrap();
        assert!(state.replay.len() <= REPLAY_EVENTS);
        assert!(state.replay_bytes <= REPLAY_BYTES);
        drop(state);
        assert!(!hub.send(&"x".repeat(MAX_EVENT_BYTES + 1)));
    }
}
