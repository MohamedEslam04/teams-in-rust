//! The "clear media cache" and "clear all cached data" commands of the disk cache
//! (`backend::cache_ops`), run through the real engine on a temporary cache dir. There is no
//! network: the engine is offline, so the re-read after clearing is skipped.

use super::*;
use std::time::{Duration, SystemTime};
use store::SavedMessage;

/// The kv key of the avatar placeholders (`backend::avatar_placeholder::KV_KEY`).
const PLACEHOLDERS_KEY: &str = "avatar_placeholders";

fn put(dir: &std::path::Path, rel: &str, len: usize) -> std::path::PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![5; len]).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    path
}

struct Setup {
    h: Harness,
    _tmp: tempfile::TempDir,
    cache: std::path::PathBuf,
}

fn setup() -> Setup {
    let h = Harness::new();
    h.engine
        .offline
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    h.engine.test_set_cache_paths(session::Paths {
        config_dir: tmp.path().join("config"),
        data_dir: tmp.path().join("data"),
        cache_dir: cache.clone(),
    });
    Setup {
        h,
        _tmp: tmp,
        cache,
    }
}

fn run(h: &Harness, cmd: Cmd) {
    h.runner()(cmd);
}

#[test]
fn clearing_the_media_cache_empties_pictures_files_and_stored_avatars_only() {
    let Setup { h, cache, .. } = setup();
    let img = put(&cache, "media/img/abc", 100);
    let voice = put(&cache, "media/v1.m4a", 100);
    let queued = put(&cache, "outbox/m1/photo.png", 100);
    let font = put(&cache, "fonts/emoji.ttf", 100);
    h.engine
        .test_set_avatar(demo::ME, crate::data::Pixels::new(4, 4));
    h.engine.test_store(|st| {
        st.upsert_user(&store::UserRow {
            mri: "8:orgid:cache-test".into(),
            display_name: "Cache Test".into(),
            email: None,
            avatar: Some(vec![1, 2, 3]),
            updated_ms: 1,
        })
        .unwrap();
        st.set_draft(BACKEND, "keep me", None).unwrap();
    });
    assert!(h.engine.test_avatar_cached(demo::ME));

    run(&h, Cmd::ClearMediaCache);

    assert!(!img.exists() && !voice.exists());
    assert!(queued.exists(), "queued uploads are never cleared");
    assert!(font.exists());
    assert!(
        !h.engine.test_avatar_cached(demo::ME),
        "the memory cache is empty"
    );
    h.engine.test_store(|st| {
        let u = st.user("8:orgid:cache-test").unwrap().unwrap();
        assert_eq!(u.avatar, None);
        assert_eq!(u.display_name, "Cache Test");
        assert_eq!(st.draft(BACKEND).unwrap().unwrap().text, "keep me");
        assert!(
            !st.messages_before(BACKEND, None, 5).unwrap().is_empty(),
            "messages are not part of the media cache"
        );
    });
}

#[test]
fn clearing_all_cached_data_keeps_what_is_the_users_own() {
    let Setup { h, cache, .. } = setup();
    let img = put(&cache, "media/img/abc", 100);
    let queued = put(&cache, "outbox/m1/photo.png", 100);
    let (kept_msg, favorite) = h.engine.test_store(|st| {
        let m = st
            .messages_before(BACKEND, None, 100)
            .unwrap()
            .pop()
            .expect("demo messages");
        st.save_message(&SavedMessage {
            conversation_id: BACKEND.into(),
            message_id: m.id.clone(),
            root_id: m.id.clone(),
            sender_name: "Sara".into(),
            preview: "saved preview".into(),
            saved_ms: 5,
        })
        .unwrap();
        st.set_favorite(BACKEND, true).unwrap();
        st.set_draft(BACKEND, "half a thought", None).unwrap();
        st.set_kv(PLACEHOLDERS_KEY, "[]").unwrap();
        (m.id, true)
    });

    run(&h, Cmd::ClearAllCache);

    assert!(!img.exists());
    assert!(queued.exists());
    h.engine.test_store(|st| {
        assert!(st.messages_before(BACKEND, None, 5).unwrap().is_empty());
        assert!(st.message(&kept_msg).unwrap().is_none());
        let c = st
            .conversation(BACKEND)
            .unwrap()
            .expect("conversation kept");
        assert_eq!(c.favorite, favorite);
        assert_eq!(c.last_preview, "");
        assert_eq!(st.draft(BACKEND).unwrap().unwrap().text, "half a thought");
        assert!(st.is_saved(BACKEND, &kept_msg).unwrap());
        assert_eq!(st.get_kv(PLACEHOLDERS_KEY).unwrap(), None);
    });
}
