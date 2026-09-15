//! 隔离集成测试用的假锁执行器：不重写 session 逻辑，只把物理锁换成内存标志。
//! 用法：rest_session_drive <events_dir>
//! 行为：投影会话 → 在 session.lock_at tick（应锁定）→ 1 秒后密码解锁 →
//! 重投影断言 ended/password、锁定时长 1 秒。
use std::path::PathBuf;

use chrono::{Duration, Utc};

struct FakeLocker {
    locked: bool,
}

impl stasis::session::LockOps for FakeLocker {
    fn is_locked(&self) -> bool {
        self.locked
    }
    fn start_lock(&mut self) -> anyhow::Result<()> {
        self.locked = true;
        Ok(())
    }
    fn stop_lock(&mut self) -> anyhow::Result<()> {
        self.locked = false;
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let events_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("usage: rest_session_drive <events_dir>"))?,
    );

    let store = stasis::protocol::EventStore::new(&events_dir);
    let mut ctrl = stasis::session::Controller::new(store);
    let sessions = ctrl.sessions(Utc::now());
    let session = sessions
        .values()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no session projected from events"))?
        .clone();
    let lock_at = session.lock_at;

    let mut locker = FakeLocker { locked: false };
    let msgs = ctrl.tick(&mut locker, lock_at);
    anyhow::ensure!(locker.locked, "tick at lock_at did not lock: {msgs:?}");

    let (ok, msg) = ctrl.unlock(&mut locker, "password", lock_at + Duration::seconds(1));
    anyhow::ensure!(ok, "password unlock failed: {msg}");
    anyhow::ensure!(!locker.locked, "still locked after unlock");

    let sessions = ctrl.sessions(lock_at + Duration::seconds(2));
    let s = sessions
        .get(&session.session_id)
        .ok_or_else(|| anyhow::anyhow!("session lost after unlock"))?;
    anyhow::ensure!(
        s.phase == stasis::session::Phase::Ended,
        "phase={:?}",
        s.phase
    );
    let seg = s
        .segments
        .first()
        .ok_or_else(|| anyhow::anyhow!("no segment"))?;
    anyhow::ensure!(
        seg.reason.as_deref() == Some("password"),
        "reason={:?}",
        seg.reason
    );
    let held = seg
        .unlocked_at
        .ok_or_else(|| anyhow::anyhow!("no unlocked_at"))?
        .signed_duration_since(seg.locked_at)
        .num_seconds();
    anyhow::ensure!(held == 1, "held {held}s, expected 1");

    println!(
        "OK: session {} ended=password, immediate self-unlock",
        s.session_id
    );
    Ok(())
}
