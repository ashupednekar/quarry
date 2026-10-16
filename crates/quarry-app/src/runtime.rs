// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use gpui::{Context, Window};

/// One long-lived runtime for all DB I/O. Postgres connection drivers live here, so it must outlive every session.
pub static DB_RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
    std::sync::LazyLock::new(|| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(4, 16));
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .enable_all()
            .build()
            .expect("failed to start DB runtime")
    });

/// Runs `fut` on the DB runtime and hands the result back to the UI entity.
pub fn run_db<T, R>(
    window: &Window,
    cx: &mut Context<T>,
    fut: impl std::future::Future<Output = R> + Send + 'static,
    on_done: impl FnOnce(&mut T, R, &mut Window, &mut Context<T>) + 'static,
) where
    T: 'static,
    R: Send + 'static,
{
    let handle = DB_RUNTIME.spawn(fut);
    cx.spawn_in(window, async move |this, cx| {
        let Ok(res) = handle.await else { return };
        this.update_in(cx, |this, window, cx| {
            on_done(this, res, window, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// Runs a streaming future on the DB runtime, receiving chunk items via an MPSC channel and
/// updating the UI entity on each chunk, followed by an on_done callback when the stream finishes.
pub fn stream_db<T, C, R, F, Fut>(
    window: &Window,
    cx: &mut Context<T>,
    make_fut: F,
    mut on_chunk: impl FnMut(&mut T, C, &mut Window, &mut Context<T>) + 'static,
    on_done: impl FnOnce(&mut T, R, &mut Window, &mut Context<T>) + 'static,
) where
    T: 'static,
    C: Send + 'static,
    R: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<C>) -> Fut + 'static,
    Fut: std::future::Future<Output = R> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let fut = make_fut(tx);
    let handle = DB_RUNTIME.spawn(fut);
    cx.spawn_in(window, async move |this, cx| {
        while let Some(chunk) = rx.recv().await {
            let res = this.update_in(cx, |this, window, cx| {
                on_chunk(this, chunk, window, cx);
                cx.notify();
            });
            if res.is_err() {
                break;
            }
        }
        let Ok(res) = handle.await else { return };
        this.update_in(cx, |this, window, cx| {
            on_done(this, res, window, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// `run_db` for callers without a window (editor change events)
pub fn run_db_bg<T, R>(
    cx: &mut Context<T>,
    fut: impl std::future::Future<Output = R> + Send + 'static,
    on_done: impl FnOnce(&mut T, R, &mut Context<T>) + 'static,
) where
    T: 'static,
    R: Send + 'static,
{
    let handle = DB_RUNTIME.spawn(fut);
    cx.spawn(async move |this, cx| {
        let Ok(res) = handle.await else { return };
        this.update(cx, |this, cx| {
            on_done(this, res, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}
