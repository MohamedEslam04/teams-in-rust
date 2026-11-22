use super::*;

/// `IRONTEAMS_CALL_DUMP=<dir>`: every raw calling push, and each SDP an incoming call exchanges, is
/// written there as it arrives (as `ironteams-cli call listen --dump` does), for protocol debugging.
/// The files hold real ids, names and addresses: keep the directory out of the repo.
pub(super) fn dump_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(compat::var_os("IRONTEAMS_CALL_DUMP").filter(|v| !v.is_empty())?);
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                tracing::info!(dir = %dir.display(), "dumping calling pushes");
                Some(dir)
            }
            Err(e) => {
                tracing::warn!(%e, "IRONTEAMS_CALL_DUMP: cannot create the directory");
                None
            }
        }
    })
    .as_deref()
}

/// `<dir>/<unix ms>-<seq>-<name>.<ext>`: sorts in arrival order.
pub(super) fn dump_path(dir: &Path, name: &str, ext: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{ms}-{n:04}-{name}.{ext}"))
}

/// Writes one push as `{url, body}` JSON into `dir`.
pub(super) fn write_push(dir: &Path, p: &CallPush) -> std::io::Result<PathBuf> {
    let path = dump_path(dir, "push", "json");
    let doc = serde_json::json!({"url": p.url, "body": p.body});
    std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)?;
    Ok(path)
}

pub(super) fn dump_push(p: &CallPush) {
    if let Some(dir) = dump_dir()
        && let Err(e) = write_push(dir, p)
    {
        tracing::warn!(%e, "IRONTEAMS_CALL_DUMP: push not written");
    }
}

/// Writes what answering a call reports (`IRONTEAMS_CALL_DUMP`): a step or a blob as `.txt`, an SDP
/// as `.sdp` (see `calling::incoming::Observe`).
pub(crate) fn dump_answer(name: &str, text: &str) {
    if let Some(dir) = dump_dir() {
        let ext = if name == "step" || name.ends_with("-blob") {
            "txt"
        } else {
            "sdp"
        };
        let _ = std::fs::write(dump_path(dir, name, ext), text);
    }
}

/// `{op, method, url, status, request, response}` of one request to the conversation service.
pub(super) fn exchange_doc(e: &calling::signaling::Exchange<'_>) -> serde_json::Value {
    serde_json::json!({
        "op": e.op,
        "method": e.method,
        "url": e.url,
        "status": e.status,
        "request": e.request,
        "response": serde_json::from_str::<serde_json::Value>(e.response)
            .unwrap_or_else(|_| serde_json::Value::String(e.response.to_owned())),
    })
}

/// Writes every request IronTeams sends to the conversation service, with its answer
/// (`IRONTEAMS_CALL_DUMP`).
pub(super) fn dump_exchange(e: &calling::signaling::Exchange<'_>) {
    if let Some(dir) = dump_dir() {
        let path = dump_path(dir, &format!("ngc-{}", e.op), "json");
        if let Ok(bytes) = serde_json::to_vec_pretty(&exchange_doc(e)) {
            let _ = std::fs::write(path, bytes);
        }
    }
}
