//! Test-only protocol relay. Drops the first armed successful COMMIT response.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub async fn start(upstream: u16) -> (u16, Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
    relay(upstream, Arc::new(AtomicBool::new(false))).await
}
pub async fn pausable(upstream: u16) -> (u16, Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
    let pause = Arc::new(AtomicBool::new(false));
    let (port, _, handle) = relay(upstream, pause.clone()).await;
    (port, pause, handle)
}
async fn relay(
    upstream: u16,
    pause: Arc<AtomicBool>,
) -> (u16, Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let handle = tokio::spawn(async move {
        while let Ok((mut client, _)) = listener.accept().await {
            let flag = flag.clone();
            let pause = pause.clone();
            tokio::spawn(async move {
                let Ok(mut server) = TcpStream::connect(("127.0.0.1", upstream)).await else {
                    return;
                };
                // StartupMessage has a length prefix but no type byte; TLS is disabled.
                let Ok(len) = client.read_u32().await else {
                    return;
                };
                if !(8..=65536).contains(&len) {
                    return;
                }
                let mut startup = vec![0; len as usize - 4];
                if client.read_exact(&mut startup).await.is_err() {
                    return;
                }
                if server.write_u32(len).await.is_err() || server.write_all(&startup).await.is_err()
                {
                    return;
                }
                let (mut cr, mut cw) = client.split();
                let (mut sr, mut sw) = server.split();
                let upstream = tokio::io::copy(&mut cr, &mut sw);
                let downstream = async {
                    loop {
                        let tag = sr.read_u8().await?;
                        let len = sr.read_u32().await?;
                        if !(4..=1_048_576).contains(&len) {
                            return Err(std::io::Error::other("test frame limit"));
                        }
                        let mut payload = vec![0; len as usize - 4];
                        sr.read_exact(&mut payload).await?;
                        while pause.load(Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                        // At CommandComplete(COMMIT), PostgreSQL has already committed.
                        if tag == b'C'
                            && payload == b"COMMIT\0"
                            && flag.swap(false, Ordering::SeqCst)
                        {
                            return Ok::<(), std::io::Error>(());
                        }
                        cw.write_u8(tag).await?;
                        cw.write_u32(len).await?;
                        cw.write_all(&payload).await?;
                    }
                };
                tokio::select! { _=upstream=>{}, _=downstream=>{} }
            });
        }
    });
    (port, armed, handle)
}
