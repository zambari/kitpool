//! The device thread: one queue of commands, run in order, with folder
//! listings and read-only device browser requests answered between steps.

use super::*;

pub(super) struct Worker {
    /// The unit this worker talks to (its MIDI input name); `None` when
    /// none is plugged in, so every request fails with "not found".
    pub(super) unit: Option<String>,
    pub(super) conn: Option<Conn>,
    pub(super) em: Emitter,
    pub(super) job: Option<Prog>,
    pub(super) rx: Receiver<Cmd>,
    /// Jobs that arrived while another one ran; they go before new commands.
    pub(super) later: VecDeque<Cmd>,
    /// File counts of parked jobs, taken as they arrive so the bar's total
    /// covers the whole queue; the job reuses them when it starts.
    pub(super) counted: HashMap<u64, Vec<(usize, u64)>>,
    /// Counting parked jobs right now (counting serves pending commands too).
    pub(super) counting: bool,
    /// Device paths the running job reads or writes (see [`involved`]).
    pub(super) busy: Vec<PathBuf>,
    /// Renames of folders the running job is using, held until it is
    /// between files (see [`Worker::apply_held`]).
    pub(super) held: VecDeque<Cmd>,
    /// Renames applied while the running job ran: `(from, to)` in order.
    /// The job maps its remaining device paths through them.
    pub(super) redirects: Vec<(PathBuf, PathBuf)>,
    /// The running transfer so far: files done (written or found already
    /// there), files skipped as already there, and same-name files of
    /// another size left alone. Reported at the end of a merge.
    pub(super) merged: (usize, usize, usize),
}

/// Device paths `task` reads or writes: what a rename must not touch to run
/// ahead of it.
pub(super) fn involved(task: &Task) -> Vec<PathBuf> {
    match task {
        Task::Transfer { dir, items, .. } => {
            let mut v = Vec::new();
            for (src, dst) in items {
                if *dir != Dir::Upload {
                    v.push(src.clone());
                }
                if *dir != Dir::Download {
                    v.push(match dir {
                        Dir::Upload => upload_target(src, dst),
                        _ => dst.join(src.file_name().unwrap_or_default()),
                    });
                }
            }
            v
        }
        Task::Rename { from, to } => vec![from.clone(), to.clone()],
        Task::Mkdir { path } => vec![path.clone()],
        Task::Trash { paths } | Task::Delete { paths } => paths.clone(),
    }
}

/// Whether `task` can run between the current job's steps: one or two quick
/// requests, like a listing, that don't touch anything the running job is
/// working on. Lets a new folder be made and named while a transfer runs.
/// A rename that touches waiting jobs still goes: they are redirected
/// (see [`redirect_queue`]).
pub(super) fn jumps_queue(task: &Task, busy: &[PathBuf]) -> bool {
    match task {
        Task::Mkdir { .. } => true,
        Task::Rename { from, to } => !busy.iter().any(|p| {
            [from, to]
                .iter()
                .any(|q| p.starts_with(q) || q.starts_with(p))
        }),
        _ => false,
    }
}

/// Queue `cmd` behind what's waiting; a job someone is waiting on
/// (`first`) goes ahead of every ordinary one, behind earlier such jobs.
pub(super) fn park(later: &mut VecDeque<Cmd>, cmd: Cmd) {
    let at = match cmd {
        Cmd::Job { first: true, .. } => later
            .iter()
            .position(|c| !matches!(c, Cmd::Job { first: true, .. }))
            .unwrap_or(later.len()),
        _ => later.len(),
    };
    later.insert(at, cmd);
}

/// `p` with `from` (or anything under it) moved to `to`, if it is there.
pub(super) fn moved(p: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    p.strip_prefix(from).ok().map(|rest| {
        if rest.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(rest)
        }
    })
}

fn move_in(p: &mut PathBuf, from: &Path, to: &Path) {
    if let Some(n) = moved(p, from, to) {
        *p = n;
    }
}

/// Point a waiting job's device paths at `to` where they were under `from`.
fn redirect_task(task: &mut Task, from: &Path, to: &Path) {
    match task {
        Task::Transfer {
            dir,
            items,
            local_names,
            ..
        } => {
            for (src, dst) in items.iter_mut() {
                if *dir != Dir::Upload {
                    if let Some(n) = moved(src, from, to) {
                        if let Some(name) = local_names.remove(src) {
                            local_names.insert(n.clone(), name);
                        }
                        *src = n;
                    }
                }
                if *dir != Dir::Download {
                    move_in(dst, from, to);
                }
            }
        }
        Task::Rename { from: f, to: t } => {
            move_in(f, from, to);
            move_in(t, from, to);
        }
        Task::Mkdir { path } => move_in(path, from, to),
        Task::Trash { paths } | Task::Delete { paths } => {
            paths.iter_mut().for_each(|p| move_in(p, from, to))
        }
    }
}

/// After `from` was renamed to `to`: redirect every waiting job, so uploads
/// queued for the old folder land in the renamed one.
pub(super) fn redirect_queue(queues: &mut [&mut VecDeque<Cmd>], from: &Path, to: &Path) {
    for q in queues.iter_mut() {
        for c in q.iter_mut() {
            if let Cmd::Job { task, .. } = c {
                redirect_task(task, from, to);
            }
        }
    }
}

/// Answer listings and API browser reads waiting in `rx` on `c`, between
/// transfer chunks or dump messages; park anything else in `later`.
/// A link failure is only reported to the read: the running job's
/// next request hits it too and handles the reconnect.
pub(super) fn serve_lists(
    c: &mut Conn,
    rx: &Receiver<Cmd>,
    later: &mut VecDeque<Cmd>,
    held: &mut VecDeque<Cmd>,
    busy: &[PathBuf],
    em: &Emitter,
) {
    c.probe();
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            Cmd::List { path, generation } => {
                let result = c
                    .list(&path)
                    .map(|v| entries(&path, v))
                    .map_err(|e| e.to_string());
                em.send(Ev::Listed {
                    device: true,
                    generation,
                    path,
                    result,
                });
            }
            Cmd::Elek(cmd) if cmd.browsing() => {
                let result = cmd.read_browser(c, em);
                em.send(Ev::Elek(result.unwrap_or_else(|e| cmd.browser_failed(e.to_string()))));
            }
            Cmd::Job { job, task, .. } if jumps_queue(&task, busy) => {
                em.send(
                    Prog {
                        id: job,
                        done: 0,
                        last: 0,
                        total: 0,
                        item: 0,
                        items: 1,
                        file: None,
                    }
                    .ev(""),
                );
                let (res, at) = match &task {
                    Task::Mkdir { path } => (c.mkdir(path), path),
                    Task::Rename { from, to } => (rename_checked(c, from, to), to),
                    _ => unreachable!("jumps_queue only lets quick tasks through"),
                };
                if let (Ok(()), Task::Rename { from, to }) = (&res, &task) {
                    redirect_queue(&mut [later, held], from, to);
                }
                let errors = res.err().map(|e| e.to_string()).into_iter().collect();
                let parent = at.parent().unwrap_or(Path::new("/")).to_path_buf();
                em.send(Ev::Finished {
                    job,
                    touched: Vec::new(),
                    device_touched: vec![parent],
                    errors,
                    cancelled: false,
                });
            }
            // Renames a folder the running job is using: done as soon as
            // the job is between files, see [`Worker::apply_held`].
            job @ Cmd::Job {
                task: Task::Rename { .. },
                ..
            } => held.push_back(job),
            job => park(later, job),
        }
    }
}

/// Progress of the running transfer.
#[derive(Clone, Copy)]
pub(super) struct Prog {
    pub(super) id: u64,
    pub(super) done: u64,
    /// `done` when progress was last sent.
    pub(super) last: u64,
    /// Estimated bytes; 0 when unknown (the bar then counts items).
    pub(super) total: u64,
    pub(super) item: usize,
    pub(super) items: usize,
    /// The file on the wire: `(phase, bytes done, file size)`.
    pub(super) file: Option<(&'static str, u64, u64)>,
}

impl Prog {
    pub(super) fn ev(&self, label: &str) -> Ev {
        Ev::Progress {
            job: self.id,
            done_bytes: self.done,
            total_bytes: self.total,
            done_items: self.item,
            total_items: self.items,
            label: label.to_owned(),
            file: self.file,
        }
    }
}

/// Audio files under local `p` (non-hidden, as an upload picks them) and their bytes.
pub(super) fn count_local(p: &Path) -> (usize, u64) {
    if !p.is_dir() {
        return (1, std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    }
    let mut total = (0, 0);
    let walk = walkdir::WalkDir::new(p)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for de in walk.flatten() {
        if de.file_type().is_file() && crate::fs_model::is_audio_path(de.path()) {
            total.0 += 1;
            total.1 += de.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

impl Worker {
    pub(super) fn run(mut self) {
        loop {
            // Jobs sent while the last one ran may include one that goes
            // ahead of the others already waiting.
            if !self.later.is_empty() {
                while let Ok(c) = self.rx.try_recv() {
                    park(&mut self.later, c);
                }
            }
            let cmd = match self.later.pop_front() {
                Some(c) => c,
                None if self.conn.is_some() => {
                    let probed = self.conn.as_ref().map_or_else(Instant::now, |c| c.probed);
                    match self
                        .rx
                        .recv_timeout(PROBE_EVERY.saturating_sub(probed.elapsed()))
                    {
                        Ok(c) => c,
                        Err(RecvTimeoutError::Timeout) => {
                            self.check_alive();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match self.rx.recv() {
                    Ok(c) => c,
                    Err(_) => return,
                },
            };
            let long = matches!(cmd, Cmd::Job { .. } | Cmd::Elek(_));
            if long {
                self.mark_busy(true);
            }
            match cmd {
                Cmd::List { path, generation } => self.list_now(path, generation),
                Cmd::Job { job, cancel, task, .. } => {
                    // Renames done before this job: its paths are already current.
                    self.redirects.clear();
                    self.busy = involved(&task);
                    self.job(job, &cancel, task);
                    self.busy.clear();
                    self.redirects.clear();
                }
                Cmd::Elek(cmd) => self.elek(cmd),
                Cmd::Release => self.release(),
            }
            if self.later.is_empty() {
                self.mark_busy(false);
            }
            // Renames held back while that ran go before the waiting jobs.
            self.apply_held();
        }
    }

    /// Switched off: close the ports, quietly (no "lost" event: the UI
    /// already folded the device panels).
    pub(super) fn release(&mut self) {
        if self.conn.take().is_some() {
            crate::log::line("device: ports closed (switched off)".to_owned());
        }
        self.set_status(Err("Switched off".into()));
    }

    /// Tell the device rows whether this unit is working.
    fn mark_busy(&self, busy: bool) {
        if let Some(u) = &self.unit {
            if super::presence::set_busy(u, busy) {
                self.em.repaint();
            }
        }
    }

    /// This worker's unit is switched off.
    pub(super) fn off(&self) -> bool {
        self.unit.as_deref().is_some_and(super::presence::unit_off)
    }

    /// Tell the UI how the link is, now and in [`super::connection_status`].
    pub(super) fn set_status(&self, s: Result<String, String>) {
        if let Some(u) = &self.unit {
            super::presence::set_connection_status(u, s.clone());
        }
        self.em.send(Ev::DeviceStatus(s));
    }

    /// Run the renames held back because they touch the running job's
    /// folders. Call only between files: nothing is open on the device.
    /// The running job then finds its remaining paths with [`Self::here`],
    /// and waiting jobs are redirected.
    pub(super) fn apply_held(&mut self) {
        while let Some(cmd) = self.held.pop_front() {
            let Cmd::Job {
                job,
                cancel,
                task: Task::Rename { from, to },
                ..
            } = cmd
            else {
                continue;
            };
            let parents: Vec<PathBuf> = [&from, &to]
                .iter()
                .map(|p| p.parent().unwrap_or(Path::new("/")).to_path_buf())
                .collect();
            if cancel.load(Ordering::Relaxed) {
                self.em.send(Ev::Finished {
                    job,
                    touched: Vec::new(),
                    device_touched: Vec::new(),
                    errors: Vec::new(),
                    cancelled: true,
                });
                continue;
            }
            let res = self.with_conn(|c| rename_checked(c, &from, &to));
            if res.is_ok() {
                self.renamed(from, to);
            }
            self.em.send(Ev::Finished {
                job,
                touched: Vec::new(),
                device_touched: parents,
                errors: res.err().into_iter().collect(),
                cancelled: false,
            });
        }
    }

    /// `from` is now `to` on the device: the running job (see [`Self::here`])
    /// and every waiting one carry on under the new name.
    pub(super) fn renamed(&mut self, from: PathBuf, to: PathBuf) {
        crate::log::line(format!(
            "renamed {} -> {}; redirecting jobs that follow",
            dev_str(&from),
            dev_str(&to)
        ));
        for p in self.busy.iter_mut() {
            move_in(p, &from, &to);
        }
        redirect_queue(&mut [&mut self.later, &mut self.held], &from, &to);
        self.redirects.push((from, to));
    }

    /// Device path `p` of the running job, after the renames done since
    /// it started.
    pub(super) fn here(&self, p: &Path) -> PathBuf {
        let mut p = p.to_path_buf();
        for (from, to) in &self.redirects {
            move_in(&mut p, from, to);
        }
        p
    }

    pub(super) fn list_now(&mut self, path: PathBuf, generation: u64) {
        let result = self.with_conn(|c| c.list(&path)).map(|v| entries(&path, v));
        self.em.send(Ev::Listed {
            device: true,
            generation,
            path,
            result,
        });
    }

    /// Drop the connection if the device's port is gone.
    pub(super) fn check_alive(&mut self) {
        if self.off() {
            if self.conn.is_some() {
                self.release();
            }
            return;
        }
        if self.conn.as_mut().is_some_and(|c| !c.probe()) {
            self.link_failed("Device disconnected".into());
        }
    }

    /// The link broke: drop the connection, and tell the UI whether the
    /// device is gone altogether (unplugged) or just stopped answering.
    pub(super) fn link_failed(&mut self, e: String) -> String {
        let gone = self
            .conn
            .take()
            .is_some_and(|c| c.dead || !port_present(&c.port));
        self.set_status(Err(e.clone()));
        if gone {
            self.em.send(Ev::DeviceLost);
        }
        e
    }

    /// Between steps: answer queued browser reads, park other jobs.
    pub(super) fn serve_pending(&mut self) {
        self.check_alive();
        // Include reads already parked by a transfer's chunk callback.
        // Keep the other jobs in their original order.
        let mut pending = std::mem::take(&mut self.later);
        while let Ok(cmd) = self.rx.try_recv() {
            pending.push_back(cmd);
        }
        while let Some(cmd) = pending.pop_front() {
            match cmd {
                Cmd::List { path, generation } => self.list_now(path, generation),
                Cmd::Elek(cmd) if cmd.browsing() => self.elek(cmd),
                // Quick enough to run between the current job's steps.
                Cmd::Job { job, cancel, task, .. } if jumps_queue(&task, &self.busy) => {
                    let n = self.redirects.len();
                    self.job(job, &cancel, task);
                    // A rename redirected the parked jobs; these too.
                    if let Some((from, to)) = self.redirects.get(n).cloned() {
                        redirect_queue(&mut [&mut pending], &from, &to);
                    }
                }
                // Touches the running job's folders: held until it is
                // between files.
                job @ Cmd::Job {
                    task: Task::Rename { .. },
                    ..
                } => self.held.push_back(job),
                Cmd::Release => self.release(),
                job => park(&mut self.later, job),
            }
        }
        self.count_parked();
    }

    /// What a job's files are counted from: its top-level paths, whether they
    /// are local (an upload), and whether the counted bytes replace the
    /// caller's guess.
    pub(super) fn count_sources(task: &Task) -> Option<(Vec<PathBuf>, bool, bool)> {
        match task {
            Task::Transfer { dir, items, .. } => {
                // An upload counts its local file sizes, which is what its
                // progress adds up to (see `write_raw`'s `worth`).
                Some((
                    items.iter().map(|(s, _)| s.clone()).collect(),
                    *dir == Dir::Upload,
                    true,
                ))
            }
            // Trash moves bytes; a delete just counts files.
            Task::Trash { paths } => Some((paths.clone(), false, true)),
            Task::Delete { paths } => Some((paths.clone(), false, false)),
            _ => None,
        }
    }

    /// Count the files of jobs waiting in `later` and put them on the bar now,
    /// rather than when each job's turn comes.
    pub(super) fn count_parked(&mut self) {
        if self.counting {
            return;
        }
        let todo: Vec<(u64, Arc<AtomicBool>, Vec<PathBuf>, bool, bool)> = self
            .later
            .iter()
            .filter_map(|c| match c {
                Cmd::Job { job, cancel, task, .. } if !self.counted.contains_key(job) => {
                    let (paths, local, bytes) = Self::count_sources(task)?;
                    Some((*job, cancel.clone(), paths, local, bytes))
                }
                _ => None,
            })
            .collect();
        if todo.is_empty() {
            return;
        }
        self.counting = true;
        for (job, cancel, paths, local, bytes) in todo {
            let counts = self.count_files(&paths, local, &cancel);
            self.em.send(Ev::Counted {
                job,
                items: counts.iter().map(|c| c.0).sum(),
                bytes: bytes.then(|| counts.iter().map(|c| c.1).sum()),
            });
            self.counted.insert(job, counts);
        }
        self.counting = false;
    }

    pub(super) fn status(&self) {
        let s = match &self.conn {
            Some(c) => Ok(format!(
                "{}{}",
                c.name,
                if c.stereo { "" } else { " (mono)" }
            )),
            None => Err("not connected".to_owned()),
        };
        self.set_status(s);
    }

    /// Run `f` on the connection, opening it first if needed. A link failure
    /// closes it again so the next call starts fresh.
    pub(super) fn with_conn<T>(&mut self, f: impl FnOnce(&mut Conn) -> R<T>) -> Result<T, String> {
        if self.off() {
            self.conn = None;
            return Err("Device switched off (click its name to connect)".into());
        }
        if self.conn.is_none() {
            match Conn::open(self.unit.as_deref()) {
                Ok(c) => {
                    self.conn = Some(c);
                    self.status();
                }
                Err(e) => {
                    self.set_status(Err(e.clone()));
                    return Err(e);
                }
            }
        }
        let c = self.conn.as_mut().expect("opened above");
        match f(c) {
            Ok(v) => Ok(v),
            Err(Fail::Link(e)) => Err(self.link_failed(e)),
            Err(Fail::Refused(e)) => Err(e),
        }
    }

    pub(super) fn progress(&mut self, label: &str, n: u64) {
        self.serve_pending();
        let Some(p) = self.job.as_mut() else { return };
        if !label.is_empty() {
            p.file = None;
        }
        p.done += n;
        if label.is_empty() && p.done - p.last < PROGRESS_EVERY {
            return;
        }
        p.last = p.done;
        let ev = p.ev(label);
        self.em.send(ev);
    }

    pub(super) fn job(&mut self, job: u64, cancel: &AtomicBool, task: Task) {
        let mut errors = Vec::new();
        let mut touched_dev: Vec<PathBuf> = Vec::new();
        let mut touched_local: Vec<PathBuf> = Vec::new();
        let touch = |v: &mut Vec<PathBuf>, p: &Path| {
            if !v.iter().any(|t| t == p) {
                v.push(p.to_path_buf());
            }
        };
        // Tells the UI this job is now running rather than waiting in line.
        // Counted while it waited: start from those totals so the bar
        // doesn't shrink back to top-level items for a moment.
        let mut counted = self.counted.remove(&job);
        let scan_bytes = Self::count_sources(&task).is_some_and(|s| s.2);
        let items = match (&counted, &task) {
            (Some(c), _) => c.iter().map(|c| c.0).sum(),
            (None, Task::Transfer { items, .. }) => items.len(),
            (None, Task::Trash { paths } | Task::Delete { paths }) => paths.len(),
            _ => 1,
        };
        let total = match (&counted, &task) {
            (Some(c), _) if scan_bytes => c.iter().map(|c| c.1).sum(),
            (_, Task::Transfer { total_bytes, .. }) => *total_bytes,
            _ => 0,
        };
        self.em.send(
            Prog {
                id: job,
                done: 0,
                last: 0,
                total,
                item: 0,
                items,
                file: None,
            }
            .ev(""),
        );
        let mut counts_for = |w: &mut Self, paths: &[PathBuf], local: bool| match counted.take() {
            Some(c) => c,
            None => {
                w.progress("Counting files…", 0);
                w.count_files(paths, local, cancel)
            }
        };

        match task {
            Task::Rename { from, to } => {
                let res = self.with_conn(|c| rename_checked(c, &from, &to));
                touch(&mut touched_dev, to.parent().unwrap_or(Path::new("/")));
                match res {
                    Ok(()) => self.renamed(from, to),
                    Err(e) => errors.push(e),
                }
            }
            Task::Mkdir { path } => {
                if let Err(e) = self.with_conn(|c| c.mkdir(&path)) {
                    errors.push(e);
                }
                touch(&mut touched_dev, path.parent().unwrap_or(Path::new("/")));
            }
            Task::Trash { paths } => {
                let trash = PathBuf::from(TRASH);
                self.job = Some(Prog {
                    id: job,
                    done: 0,
                    last: 0,
                    total: 0,
                    item: 0,
                    items: paths.len(),
                    file: None,
                });
                let counts = counts_for(self, &paths, false);
                self.set_totals(&counts, true);
                let mut before = (0, 0);
                for (i, p) in paths.into_iter().enumerate() {
                    self.catch_up(&mut before, counts.get(i.wrapping_sub(1)), true);
                    if p.starts_with(&trash) {
                        errors.push(format!("{} is already in {TRASH}", dev_str(&p)));
                        continue;
                    }
                    let target = self.with_conn(|c| {
                        if !c
                            .list(Path::new("/"))?
                            .iter()
                            .any(|e| e.is_dir && e.name == "trash")
                        {
                            c.mkdir(&trash)?;
                        }
                        let taken: HashSet<String> =
                            c.list(&trash)?.into_iter().map(|e| e.name).collect();
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let name = if taken.contains(&name) {
                            unique_name(&taken, &name)
                        } else {
                            name
                        };
                        Ok(trash.join(name))
                    });
                    let res = target.and_then(|t| {
                        let e = self.stat(&p)?;
                        self.progress(&e.name, 0);
                        self.relocate(&p, &t, e.is_dir, e.size as u64, cancel)
                    });
                    match res {
                        Ok(_) => touch(&mut touched_dev, p.parent().unwrap_or(Path::new("/"))),
                        Err(e) => errors.push(e),
                    }
                }
                self.job = None;
                touch(&mut touched_dev, &trash);
                touch(&mut touched_dev, Path::new("/"));
            }
            Task::Delete { paths } => {
                self.job = Some(Prog {
                    id: job,
                    done: 0,
                    last: 0,
                    total: 0,
                    item: 0,
                    items: paths.len(),
                    file: None,
                });
                let counts = counts_for(self, &paths, false);
                self.set_totals(&counts, false);
                let mut before = (0, 0);
                for (i, p) in paths.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    self.catch_up(&mut before, counts.get(i.wrapping_sub(1)), false);
                    let res = self
                        .stat(&p)
                        .and_then(|e| self.remove(&p, e.is_dir, cancel));
                    if let Err(e) = res {
                        errors.push(e);
                    }
                    touch(&mut touched_dev, p.parent().unwrap_or(Path::new("/")));
                }
                self.job = None;
            }
            Task::Transfer {
                dir,
                items,
                policy,
                mono,
                total_bytes,
                local_names,
            } => {
                let n = items.len();
                crate::log::line(format!("job {job}: {dir:?} {n} item(s), policy {policy:?}"));
                for (src, dst) in &items {
                    crate::log::line(format!("  {} -> {}", src.display(), dst.display()));
                }
                self.job = Some(Prog {
                    id: job,
                    done: 0,
                    last: 0,
                    total: total_bytes,
                    item: 0,
                    items: n,
                    file: None,
                });
                self.merged = (0, 0, 0);
                let srcs: Vec<PathBuf> = items.iter().map(|(s, _)| s.clone()).collect();
                let counts = counts_for(self, &srcs, dir == Dir::Upload);
                self.set_totals(&counts, scan_bytes);
                let mut before = (0, 0);
                for (i, (src0, dst_dir)) in items.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    // Between items: a held rename can go now, and this
                    // item follows it.
                    self.apply_held();
                    let dst_dir = match dir {
                        Dir::Download => dst_dir,
                        _ => self.here(&dst_dir),
                    };
                    let src = match dir {
                        Dir::Upload => src0.clone(),
                        _ => self.here(&src0),
                    };
                    self.catch_up(&mut before, counts.get(i.wrapping_sub(1)), scan_bytes);
                    self.progress(
                        &src.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        0,
                    );
                    let res = match dir {
                        Dir::Move => self.move_item(&src, &dst_dir, policy, cancel),
                        Dir::Upload => self.ensure_dev_dir(&dst_dir).and_then(|()| {
                            self.upload(&src, &dst_dir, policy, mono, cancel, &mut errors)
                        }),
                        Dir::Download => {
                            let name = local_names.get(&src0).map(String::as_str);
                            self.download(&src, &dst_dir, name, policy, cancel, &mut errors)
                        }
                        Dir::Copy => self.copy(&src, &dst_dir, policy, cancel, &mut errors),
                    };
                    if let Err(e) = res {
                        errors.push(e);
                    }
                    match dir {
                        Dir::Download => touch(&mut touched_local, &dst_dir),
                        _ => touch(&mut touched_dev, &dst_dir),
                    }
                    if dir == Dir::Move {
                        touch(&mut touched_dev, src.parent().unwrap_or(Path::new("/")));
                    }
                }
                if policy == Policy::Merge {
                    let (done, dupes, clashes) = self.merged;
                    let added = done.saturating_sub(dupes);
                    let mut text = format!("Merged: {added} added");
                    if dupes > 0 {
                        text += &format!(", {dupes} already there skipped");
                    }
                    if clashes > 0 {
                        text += &format!(
                            ", {clashes} with the same name but a different size not copied"
                        );
                    }
                    self.em.send(Ev::Note { job, text });
                }
                self.job = None;
            }
        }

        let cancelled = cancel.load(Ordering::Relaxed);
        if cancelled {
            errors.retain(|e| !e.ends_with("cancelled"));
        }
        // Relist folders where they are now.
        let mut touched_dev: Vec<PathBuf> = touched_dev.iter().map(|p| self.here(p)).collect();
        touched_dev.dedup();
        self.em.send(Ev::Finished {
            job,
            touched: touched_local,
            device_touched: touched_dev,
            errors,
            cancelled,
        });
    }

    /// Files under each of `paths` and their bytes, so the bar can count
    /// files rather than top-level items. `local`: paths on disk (an
    /// upload), filtered the way [`Self::upload`] picks files.
    pub(super) fn count_files(
        &mut self,
        paths: &[PathBuf],
        local: bool,
        cancel: &AtomicBool,
    ) -> Vec<(usize, u64)> {
        let mut listings: HashMap<PathBuf, Vec<(String, bool, u64)>> = HashMap::new();
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            if local {
                out.push(count_local(p));
                continue;
            }
            // One listing per parent folder, not one per item.
            let parent = p.parent().unwrap_or(Path::new("/")).to_path_buf();
            if !listings.contains_key(&parent) {
                let l = self.with_conn(|c| c.list(&parent)).unwrap_or_default();
                listings.insert(
                    parent.clone(),
                    l.into_iter()
                        .map(|e| (e.name, e.is_dir, e.size as u64))
                        .collect(),
                );
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let found = listings[&parent]
                .iter()
                .find(|e| e.0 == name)
                .map(|e| (e.1, e.2));
            out.push(match found {
                None => (0, 0),
                Some((false, size)) => (1, size),
                Some((true, _)) => self.count_dir(p, cancel),
            });
        }
        out
    }

    pub(super) fn count_dir(&mut self, p: &Path, cancel: &AtomicBool) -> (usize, u64) {
        let Ok(kids) = self.with_conn(|c| c.list(p)) else {
            return (0, 0);
        };
        let mut total = (0, 0);
        for k in kids {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            self.serve_pending();
            let (n, b) = if k.is_dir {
                self.count_dir(&p.join(&k.name), cancel)
            } else {
                (1, k.size as u64)
            };
            total.0 += n;
            total.1 += b;
        }
        total
    }

    /// Put the counted files (and, with `bytes`, their size) on the bar.
    pub(super) fn set_totals(&mut self, counts: &[(usize, u64)], bytes: bool) {
        let Some(p) = self.job.as_mut() else { return };
        p.items = counts.iter().map(|c| c.0).sum();
        if bytes {
            p.total = counts.iter().map(|c| c.1).sum();
        }
        let ev = p.ev("");
        self.em.send(ev);
    }

    /// Before the next top-level item: count the previous one (`prev`) as
    /// fully done, so skipped files or a folder renamed in one go still add up.
    pub(super) fn catch_up(
        &mut self,
        before: &mut (usize, u64),
        prev: Option<&(usize, u64)>,
        bytes: bool,
    ) {
        let Some(&(n, b)) = prev else { return };
        before.0 += n;
        before.1 += b;
        let Some(p) = self.job.as_mut() else { return };
        p.item = p.item.max(before.0);
        if bytes {
            p.done = p.done.max(before.1);
        }
    }

    /// One file finished: tick the file count.
    pub(super) fn file_done(&mut self) {
        self.merged.0 += 1;
        let Some(p) = self.job.as_mut() else { return };
        p.item += 1;
        p.last = p.done;
        let ev = p.ev("");
        self.em.send(ev);
    }
}
