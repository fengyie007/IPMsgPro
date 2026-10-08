use super::*;
use crate::scan::{ScanOptions, ScanPhase, ScanPlan, ScanStatus};
use std::collections::HashSet;
use std::sync::atomic::AtomicU64;

struct Data {
    status: ScanStatus,
    probed: HashSet<u32>,
    found: HashSet<SocketAddrV4>,
    last_event: Instant,
}
#[derive(Default)]
struct Control {
    task: Option<JoinHandle<()>>,
    cancel: Option<watch::Sender<bool>>,
}
pub(super) struct Scanner {
    data: Mutex<Data>,
    control: tokio::sync::Mutex<Control>,
    next: AtomicU64,
}
impl Default for Scanner {
    fn default() -> Self {
        Self {
            data: Mutex::new(Data {
                status: ScanStatus::default(),
                probed: HashSet::new(),
                found: HashSet::new(),
                last_event: Instant::now(),
            }),
            control: tokio::sync::Mutex::new(Control::default()),
            next: AtomicU64::new(1),
        }
    }
}
impl Network {
    pub fn scan_status(&self) -> ScanStatus {
        self.scan
            .data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }
    fn scan_event(&self, force: bool) {
        let status = {
            let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
            if !force && data.last_event.elapsed() < Duration::from_millis(100) {
                return;
            }
            data.last_event = Instant::now();
            data.status.clone()
        };
        let name = if status.state.active() {
            "network.scan_progress"
        } else {
            "network.scan_complete"
        };
        // A slow UI must not stall probes, normal UDP reception, or cancellation.
        // scan_status is authoritative; the settings page also refreshes it periodically.
        let _ = self.events.try_send(Event {
            event: name.into(),
            payload: json!(status),
        });
    }
    pub(super) fn scan_response(&self, address: SocketAddrV4) {
        let changed = {
            let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(data.status.state, ScanPhase::Running | ScanPhase::Waiting)
                || address.port() != data.status.port
                || !data.probed.contains(&u32::from(*address.ip()))
            {
                false
            } else if data.found.insert(address) {
                data.status.found = data.found.len() as u32;
                data.status.revision += 1;
                true
            } else {
                false
            }
        };
        if changed {
            self.scan_event(false);
        }
    }
    pub async fn start_scan(self: &Arc<Self>, options: ScanOptions) -> Result<ScanStatus, String> {
        self.start_scan_inner(options, false).await
    }
    pub(super) async fn start_scan_inner(
        self: &Arc<Self>,
        options: ScanOptions,
        skip_existing: bool,
    ) -> Result<ScanStatus, String> {
        let plan = options.plan()?;
        if plan.total == 0 {
            return Err("请至少配置一个扫描范围".into());
        }
        let mut control = self.scan.control.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let previous = self.scan_status();
        if previous.state.active() {
            if skip_existing {
                return Ok(previous);
            }
            return Err("已有扫描正在进行，请先取消".into());
        }
        if let Some(task) = control.task.take() {
            let _ = task.await;
        }
        let id = self.scan.next.fetch_add(1, Ordering::Relaxed);
        let status = ScanStatus {
            scan_id: id,
            revision: 1,
            state: ScanPhase::Running,
            current: 0,
            total: plan.total,
            found: 0,
            failed_sends: 0,
            skipped: 0,
            port: plan.port,
            delay_ms: plan.delay_ms,
            ranges: plan
                .ranges
                .iter()
                .map(|&(a, b)| format!("{}-{}", Ipv4Addr::from(a), Ipv4Addr::from(b)))
                .collect(),
            error: None,
        };
        {
            let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
            data.status = status.clone();
            data.probed.clear();
            data.found.clear();
        }
        let (cancel, receiver) = watch::channel(false);
        control.cancel = Some(cancel);
        self.scan_event(true);
        let network = self.clone();
        control.task = Some(tokio::spawn(async move {
            network.scan_worker(id, plan, receiver).await;
        }));
        Ok(status)
    }
    fn finish_scan(&self, id: u64, phase: ScanPhase, error: Option<String>) {
        {
            let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
            if data.status.scan_id != id || !data.status.state.active() {
                return;
            }
            data.status.state = if data.status.state == ScanPhase::Cancelling {
                ScanPhase::Cancelled
            } else {
                phase
            };
            if error.is_some() {
                data.status.error = error;
            }
            data.status.revision += 1;
        }
        self.scan_event(true);
    }
    async fn scan_worker(
        self: Arc<Self>,
        id: u64,
        plan: ScanPlan,
        mut cancel: watch::Receiver<bool>,
    ) {
        let mut last_event = Instant::now();
        for ip in plan.ranges.iter().flat_map(|&(start, end)| start..=end) {
            if *cancel.borrow() || self.stopping.load(Ordering::Acquire) {
                self.finish_scan(id, ScanPhase::Cancelled, None);
                return;
            }
            let local = self.local();
            let skip = local.port == plan.port && local.ip == Ipv4Addr::from(ip).to_string();
            {
                let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
                if data.status.scan_id != id {
                    return;
                }
                data.status.current += 1;
                data.status.revision += 1;
                if skip {
                    data.status.skipped += 1;
                } else {
                    data.probed.insert(ip);
                }
            }
            if !skip {
                let wire = match self.wire(
                    IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
                    &local.nickname,
                    Some(&local.group),
                ) {
                    Ok(wire) => wire,
                    Err(error) => {
                        self.finish_scan(id, ScanPhase::Failed, Some(error));
                        return;
                    }
                };
                let target = SocketAddrV4::new(Ipv4Addr::from(ip), plan.port);
                let result = tokio::select! {
                    _=cancel.changed()=>{self.finish_scan(id,ScanPhase::Cancelled,None);return;},
                    result=tokio::time::timeout(Duration::from_secs(1),self.socket.send_to(&wire,target))=>result.map_err(|_|"UDP探测发送超时".to_owned()).and_then(|result|result.map(|_|()).map_err(|e|e.to_string()))
                };
                if let Err(error) = result {
                    let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
                    data.status.failed_sends += 1;
                    data.status.error = Some(error);
                    data.status.revision += 1;
                }
            }
            if last_event.elapsed() >= Duration::from_millis(100) {
                self.scan_event(false);
                last_event = Instant::now();
            }
            if self.scan_status().current < plan.total {
                tokio::select! {_=cancel.changed()=>{self.finish_scan(id,ScanPhase::Cancelled,None);return;},
                _=tokio::time::sleep(Duration::from_millis(u64::from(plan.delay_ms)))=>{}}
            }
        }
        if *cancel.borrow() {
            self.finish_scan(id, ScanPhase::Cancelled, None);
            return;
        }
        {
            let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
            if data.status.state == ScanPhase::Cancelling {
                drop(data);
                self.finish_scan(id, ScanPhase::Cancelled, None);
                return;
            }
            data.status.state = ScanPhase::Waiting;
            data.status.revision += 1;
        }
        self.scan_event(true);
        tokio::select! {_=cancel.changed()=>{self.finish_scan(id,ScanPhase::Cancelled,None);return;},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
        let status = self.scan_status();
        let failed =
            status.failed_sends > 0 && status.failed_sends + status.skipped == status.current;
        self.finish_scan(
            id,
            if failed {
                ScanPhase::Failed
            } else {
                ScanPhase::Completed
            },
            None,
        );
    }
    pub async fn cancel_scan(&self, id: u64) -> Result<ScanStatus, String> {
        self.stop_scan(Some(id)).await
    }
    pub(super) async fn shutdown_scan(&self) {
        let _ = self.stop_scan(None).await;
    }
    async fn stop_scan(&self, expected: Option<u64>) -> Result<ScanStatus, String> {
        let mut control = self.scan.control.lock().await;
        let status = self.scan_status();
        if expected.is_some_and(|id| id != status.scan_id) {
            return Err("扫描任务已变更，请刷新状态".into());
        }
        if status.state.active() {
            {
                let mut data = self.scan.data.lock().unwrap_or_else(|e| e.into_inner());
                // Completion may have won before we acquired the state lock.
                if data.status.state.active() {
                    data.status.state = ScanPhase::Cancelling;
                    data.status.revision += 1;
                }
            }
            if let Some(cancel) = control.cancel.take() {
                cancel.send_replace(true);
            }
            self.scan_event(true);
        }
        if let Some(mut task) = control.task.take() {
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        self.finish_scan(status.scan_id, ScanPhase::Cancelled, None);
        Ok(self.scan_status())
    }
}
