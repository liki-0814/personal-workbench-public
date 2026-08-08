//! 终端 TUI 前端（ratatui inline 模式）
//!
//! - 底部嵌入式输入框（不接管全屏，保留正常 scrollback）
//! - 输入 `/` 自动浮出 slash 命令菜单（上下方向键选择，Enter 确认，Esc 取消）
//! - LLM 流式文本与工具卡片通过 `Terminal::insert_before` 写到 inline 区上方
//! - 用户连续输入入队，agent 完成当前 turn 后自动取下一条
//! - Ctrl-C：流式中=取消 turn；idle 时=退出
//!
//! 架构：
//! - TUI 任务（main task）：持有终端，渲染，吃键盘事件，吃 UI 事件
//! - Agent 任务（spawn）：持有 messages/session/usage，按 `UserSubmission` 串行处理
//! - 通信：`mpsc::Sender<UserSubmission>` 给 agent 投递；`mpsc::UnboundedSender<UiEvent>` 反向通知

mod diff;
mod image;
mod picker;

pub use picker::MenuSources;

use std::collections::VecDeque;
use std::io::{self, Stdout, Write};
use std::time::Duration;

use crate::runtime::session::estimate_text_tokens;
use picker::MenuState;

use anyhow::Result;
use clap::ValueEnum;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::QueueableCommand;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TuiMode {
    Fullscreen,
    Inline,
}

/// agent 任务收到的工作项
#[derive(Debug)]
pub enum UserSubmission {
    /// 用户输入一段文本（slash 命令或自然语言）
    Text(String),
    /// 通知 agent 退出
    Shutdown,
}

/// agent → TUI 事件
#[derive(Debug)]
pub enum UiEvent {
    /// Agent 即将开始一个 turn；携带本 turn 的取消令牌（TUI 在 Ctrl-C 时调 cancel()）
    TurnStart { cancel_token: CancellationToken },
    /// 用户消息回显（"▸ hello"），TUI 将其插入 scrollback
    UserEcho(String),
    /// 系统/反馈消息（slash 命令输出、错误等）
    SystemLine(String),
    /// 即将调用 LLM（用于切「思考中」占位）
    Thinking,
    /// 流式文本增量
    AssistantDelta(String),
    /// 一次 LLM 响应结束（其后可能还有工具+下一轮）
    AssistantEnd,
    /// A pathological partial stream was discarded before one resample.
    StreamRetry(String),
    /// 工具调用卡片
    ToolCall { name: String, args: String },
    /// 工具执行结果
    ToolResult { result: String, is_error: bool },
    /// 工具生成的图片。TUI 在支持的终端中可按需预览，否则展示安全降级信息。
    ToolImage { url: String, alt: String },
    /// 权限交互请求（同步等待用户）
    PermissionRequest {
        name: String,
        args: String,
        reply: oneshot::Sender<bool>,
    },
    /// turn 完成（agent 进入 idle）
    TurnComplete,
    /// 用量更新（用于状态栏显示）
    UsageUpdate { used: u32, window: u32 },
    /// agent 主动报告 yolo / model / session name 等元状态变化
    Status(StatusUpdate),
    /// 致命错误（agent 任务异常）
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct StatusUpdate {
    pub model: Option<String>,
    pub yolo: Option<bool>,
    pub permission_mode: Option<String>,
    pub session_name: Option<String>,
    /// 当前 active provider 名（用于 /model 菜单选 model 列表）
    pub active_provider: Option<String>,
}

/// 待用户回应的权限请求
struct PendingPermission {
    name: String,
    args: String,
    reply: Option<oneshot::Sender<bool>>,
}

struct App {
    input: String,
    /// 字节级光标位置（在 input 中）
    cursor_byte: usize,
    menu: MenuState,
    /// 用户尚未发给 agent 的待处理输入
    queue: VecDeque<String>,
    /// agent 是否正在跑 turn
    in_progress: bool,
    /// 当前 LLM 流式累积文本（只在 inline 区预览，结束时一次性 insert_before）
    streaming_buf: String,
    /// Fullscreen 下已经越过安全 Markdown 边界的冻结行。仍属于 live region，
    /// AssistantEnd 前不会进入 timeline/scrollback。
    streaming_frozen: Vec<Line<'static>>,
    streaming_frozen_tokens: u32,
    /// AssistantEnd only marks the live block finalized. The block is moved
    /// to scrollback at the commit frontier, never while user input is pending.
    streaming_finalized: bool,
    /// 等待用户 y/n 的权限请求
    pending_perm: Option<PendingPermission>,
    /// 当前进行中 turn 的取消令牌（Ctrl-C 时触发）
    active_cancel: Option<CancellationToken>,
    /// 状态栏字段
    model: String,
    used_tokens: u32,
    window_tokens: u32,
    yolo: bool,
    permission_mode: String,
    session_name: String,
    /// 菜单数据源（启动期注入）
    sources: MenuSources,
    /// 退出标志
    should_quit: bool,
    /// 已提交输入的历史（最旧在前，最新在后），用于上下箭头浏览
    history: VecDeque<String>,
    /// 当前在 history 中浏览的位置；None 表示停留在当前编辑缓冲
    history_pos: Option<usize>,
    /// 进入历史浏览前的当前输入快照，按 Down 退出历史时恢复
    input_snapshot: String,
    mode: TuiMode,
    /// Fullscreen mode owns its scrollback so it can reflow it after resize.
    timeline: Vec<Line<'static>>,
    /// Number of logical lines kept below the visible viewport.
    scroll_from_bottom: usize,
    diff: Option<diff::DiffDocument>,
    diff_scroll: u16,
    images: Vec<image::ImageArtifact>,
    image_preview: Option<usize>,
    image_dirty: bool,
}

const HISTORY_CAP: usize = 200;

impl App {
    fn new(
        model: String,
        window_tokens: u32,
        session_name: String,
        sources: MenuSources,
        mode: TuiMode,
    ) -> Self {
        Self {
            input: String::new(),
            cursor_byte: 0,
            menu: MenuState::default(),
            queue: VecDeque::new(),
            in_progress: false,
            streaming_buf: String::new(),
            streaming_frozen: Vec::new(),
            streaming_frozen_tokens: 0,
            streaming_finalized: false,
            pending_perm: None,
            active_cancel: None,
            model,
            used_tokens: 0,
            window_tokens,
            yolo: false,
            permission_mode: "risk".to_string(),
            session_name,
            sources,
            should_quit: false,
            history: VecDeque::with_capacity(HISTORY_CAP),
            history_pos: None,
            input_snapshot: String::new(),
            mode,
            timeline: Vec::new(),
            scroll_from_bottom: 0,
            diff: None,
            diff_scroll: 0,
            images: Vec::new(),
            image_preview: None,
            image_dirty: false,
        }
    }

    /// Push a freshly-submitted line into history (dedup against immediate prev).
    fn history_push(&mut self, line: &str) {
        if self.history.back().map(|s| s.as_str()) == Some(line) {
            return; // skip exact dup of last
        }
        if self.history.len() >= HISTORY_CAP {
            self.history.pop_front();
        }
        self.history.push_back(line.to_string());
        self.history_pos = None;
    }

    /// Up-arrow: move toward older entries.
    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_pos = match self.history_pos {
            None => {
                self.input_snapshot = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0, // already oldest
            Some(p) => p - 1,
        };
        self.history_pos = Some(new_pos);
        self.input = self.history[new_pos].clone();
        self.cursor_byte = self.input.len();
    }

    /// Down-arrow: move toward newer entries; past the newest restores the
    /// pre-history-edit buffer.
    fn history_next(&mut self) {
        let Some(p) = self.history_pos else { return };
        if p + 1 >= self.history.len() {
            // Step past the newest entry → return to live buffer
            self.history_pos = None;
            self.input = std::mem::take(&mut self.input_snapshot);
            self.cursor_byte = self.input.len();
        } else {
            self.history_pos = Some(p + 1);
            self.input = self.history[p + 1].clone();
            self.cursor_byte = self.input.len();
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_byte, c);
        self.cursor_byte += c.len_utf8();
        self.menu.refresh(&self.input, &self.sources);
    }

    fn insert_text(&mut self, text: &str) {
        self.input.insert_str(self.cursor_byte, text);
        self.cursor_byte += text.len();
        self.menu.refresh(&self.input, &self.sources);
    }

    fn delete_char_before(&mut self) {
        if self.cursor_byte == 0 {
            return;
        }
        let new_pos = self.input[..self.cursor_byte]
            .grapheme_indices(true)
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.input.replace_range(new_pos..self.cursor_byte, "");
        self.cursor_byte = new_pos;
        self.menu.refresh(&self.input, &self.sources);
    }

    fn move_cursor_left(&mut self) {
        if self.cursor_byte == 0 {
            return;
        }
        self.cursor_byte = self.input[..self.cursor_byte]
            .grapheme_indices(true)
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
    }

    fn move_cursor_right(&mut self) {
        if self.cursor_byte >= self.input.len() {
            return;
        }
        self.cursor_byte += self.input[self.cursor_byte..]
            .graphemes(true)
            .next()
            .map(str::len)
            .unwrap_or(0);
    }

    fn home(&mut self) {
        self.cursor_byte = 0;
    }

    fn end(&mut self) {
        self.cursor_byte = self.input.len();
    }

    fn cursor_visual_position(&self) -> (u16, u16) {
        // 用 unicode-width 算实际终端列宽（CJK = 2 列，emoji = 2 列，控制符 = 0 列）
        let before = &self.input[..self.cursor_byte];
        let row = before.bytes().filter(|byte| *byte == b'\n').count() as u16;
        let col = UnicodeWidthStr::width(before.rsplit('\n').next().unwrap_or("")) as u16;
        (row, col)
    }
}

/// inline viewport 行数
const VIEWPORT_HEIGHT: u16 = 9;

pub async fn run(
    input_tx: mpsc::Sender<UserSubmission>,
    mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    initial_status: StatusUpdate,
    window_tokens: u32,
    sources: MenuSources,
    mode: TuiMode,
) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let _restore = TerminalRestore { mode };
    if mode == TuiMode::Fullscreen {
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
    } else {
        execute!(io::stdout(), EnableBracketedPaste)?;
    }
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: if mode == TuiMode::Fullscreen {
                Viewport::Fullscreen
            } else {
                Viewport::Inline(VIEWPORT_HEIGHT)
            },
        },
    )?;

    let mut app = App::new(
        initial_status.model.unwrap_or_else(|| "未配置".to_string()),
        window_tokens,
        initial_status
            .session_name
            .unwrap_or_else(|| "default".to_string()),
        sources,
        mode,
    );
    app.yolo = initial_status.yolo.unwrap_or(false);

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));

    let result = (async {
        draw_frame(&mut terminal, &app)?;
        loop {
            let mut redraw = true;
            tokio::select! {
                biased;

                // 用户键盘事件
                Some(Ok(ev)) = events.next() => {
                    handle_event(&mut app, &input_tx, ev, terminal.size()?.into()).await?;
                }

                // agent → TUI 事件
                Some(ui_ev) = ui_rx.recv() => {
                    handle_ui_event(&mut app, &mut terminal, ui_ev).await?;
                }

                _ = tick.tick() => {
                    redraw = app.in_progress;
                }
            }

            // 派发队列：agent 空闲且有待发消息
            if !app.in_progress {
                if let Some(next) = app.queue.pop_front() {
                    app.in_progress = true;
                    let _ = input_tx.send(UserSubmission::Text(next)).await;
                    redraw = true;
                }
            }

            if redraw {
                draw_frame(&mut terminal, &app)?;
                if app.image_dirty {
                    if let Some(index) = app.image_preview {
                        if let Some(image) = app.images.get(index) {
                            image.render(terminal.size()?.width.saturating_sub(6))?;
                        }
                    }
                    app.image_dirty = false;
                }
            }

            if app.should_quit {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    if mode == TuiMode::Inline {
        let _ = terminal.clear();
    }
    let _ = terminal.show_cursor();

    result
}

fn draw_frame(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &App) -> Result<()> {
    terminal.backend_mut().queue(BeginSynchronizedUpdate)?;
    let draw_result = terminal.draw(|frame| draw(frame, app)).map(|_| ());
    terminal.backend_mut().queue(EndSynchronizedUpdate)?;
    terminal.backend_mut().flush()?;
    draw_result?;
    Ok(())
}

struct TerminalRestore {
    mode: TuiMode,
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        image::clear_terminal_images();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, EndSynchronizedUpdate);
        if self.mode == TuiMode::Fullscreen {
            let _ = execute!(
                stdout,
                DisableMouseCapture,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
        } else {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

async fn handle_event(
    app: &mut App,
    input_tx: &mpsc::Sender<UserSubmission>,
    ev: Event,
    terminal_area: Rect,
) -> Result<()> {
    match ev {
        Event::Paste(text) => {
            app.insert_text(&text.replace("\r\n", "\n"));
            return Ok(());
        }
        Event::Mouse(mouse) if app.mode == TuiMode::Fullscreen => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if app.menu.active() {
                        app.menu.move_up();
                    } else {
                        app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if app.menu.active() {
                        app.menu.move_down();
                    } else {
                        app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(3);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if app.menu.active() => {
                    let area = fullscreen_menu_area(terminal_area, app);
                    if mouse.column > area.x
                        && mouse.column < area.right().saturating_sub(1)
                        && mouse.row > area.y
                        && mouse.row < area.bottom().saturating_sub(1)
                    {
                        let (start, end) =
                            menu_visible_range(app, area.height.saturating_sub(2) as usize);
                        let index = start + mouse.row.saturating_sub(area.y + 1) as usize;
                        if index < end {
                            app.menu.selected = index;
                            if let Some(item) = app.menu.current().cloned() {
                                app.input = format!("{} ", item.replacement);
                                app.cursor_byte = app.input.len();
                                app.menu.refresh(&app.input, &app.sources);
                            }
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        Event::Resize(_, _) => {
            app.image_dirty = app.image_preview.is_some();
            return Ok(());
        }
        _ => {}
    }

    let Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        ..
    }) = ev
    else {
        return Ok(());
    };
    if kind != KeyEventKind::Press {
        return Ok(());
    }

    // 优先处理待回应的权限请求
    if app.pending_perm.is_some() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(mut p) = app.pending_perm.take() {
                    if let Some(tx) = p.reply.take() {
                        let _ = tx.send(true);
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let Some(mut p) = app.pending_perm.take() {
                    if let Some(tx) = p.reply.take() {
                        let _ = tx.send(false);
                    }
                }
            }
            _ => {}
        }
        return Ok(());
    }

    if app.diff.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.diff = None,
            KeyCode::Up => app.diff_scroll = app.diff_scroll.saturating_sub(1),
            KeyCode::Down => app.diff_scroll = app.diff_scroll.saturating_add(1),
            KeyCode::PageUp => app.diff_scroll = app.diff_scroll.saturating_sub(10),
            KeyCode::PageDown => app.diff_scroll = app.diff_scroll.saturating_add(10),
            KeyCode::Home => app.diff_scroll = 0,
            _ => {}
        }
        return Ok(());
    }

    if app.image_preview.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => {
                app.image_preview = None;
                image::clear_terminal_images();
            }
            _ => {}
        }
        return Ok(());
    }

    match (code, modifiers) {
        // Ctrl-C：流式中 = 取消当前 turn；idle = 退出
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            if app.in_progress {
                if let Some(token) = app.active_cancel.as_ref() {
                    token.cancel();
                }
            } else {
                request_shutdown(app, input_tx);
            }
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => {
            request_shutdown(app, input_tx);
        }
        (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
            open_diff(app).await;
        }
        (KeyCode::Char('i'), KeyModifiers::NONE) if !app.images.is_empty() => {
            app.image_preview = Some(app.images.len() - 1);
            app.image_dirty = true;
        }

        (KeyCode::PageUp, _) if app.mode == TuiMode::Fullscreen => {
            app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(10);
        }
        (KeyCode::PageDown, _) if app.mode == TuiMode::Fullscreen => {
            app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(10);
        }
        (KeyCode::End, KeyModifiers::CONTROL) if app.mode == TuiMode::Fullscreen => {
            app.scroll_from_bottom = 0;
        }

        // 菜单导航
        (KeyCode::Up, _) if app.menu.active() => app.menu.move_up(),
        (KeyCode::Down, _) if app.menu.active() => app.menu.move_down(),
        // History navigation when no menu is open
        (KeyCode::Up, _) => app.history_prev(),
        (KeyCode::Down, _) => app.history_next(),
        (KeyCode::Esc, _) if app.menu.active() => {
            app.menu.items.clear();
        }
        (KeyCode::Tab, _) | (KeyCode::Enter, _) if app.menu.active() => {
            // 选中菜单项：用 replacement + 空格替换 input（用户可以继续敲参数或 Enter 提交）
            if let Some(item) = app.menu.current().cloned() {
                app.input = format!("{} ", item.replacement);
                app.cursor_byte = app.input.len();
                // 选中后立刻刷新一次：参数菜单可能要切换到下一级
                app.menu.refresh(&app.input, &app.sources);
            }
        }

        // 编辑
        (KeyCode::Enter, m)
            if m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) && !app.menu.active() =>
        {
            app.insert_char('\n');
        }
        (KeyCode::Char('j'), KeyModifiers::CONTROL) if !app.menu.active() => {
            app.insert_char('\n');
        }
        (KeyCode::Char(c), m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
            app.insert_char(c);
        }
        (KeyCode::Backspace, _) => app.delete_char_before(),
        (KeyCode::Left, _) => app.move_cursor_left(),
        (KeyCode::Right, _) => app.move_cursor_right(),
        (KeyCode::Home, _) => app.home(),
        (KeyCode::End, _) => app.end(),

        // 提交
        (KeyCode::Enter, _) => {
            let line = app.input.trim().to_string();
            if !line.is_empty() {
                app.history_push(&line);
                app.input.clear();
                app.cursor_byte = 0;
                app.menu.items.clear();
                app.menu.selected = 0;

                if is_exit_command(&line) {
                    request_shutdown(app, input_tx);
                } else if line == "/diff" {
                    open_diff(app).await;
                } else if app.in_progress {
                    // 入队，UI 显示 queue 数；状态栏会反映
                    app.queue.push_back(line);
                } else {
                    // 直接派发给 agent
                    app.in_progress = true;
                    let _ = input_tx.send(UserSubmission::Text(line)).await;
                }
            }
        }

        _ => {}
    }
    Ok(())
}

fn is_exit_command(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "/exit" | "/quit"
    )
}

/// Exit commands must take precedence over an active turn and queued prompts.
/// `try_send` deliberately avoids making terminal shutdown wait for a full queue;
/// the owner of the agent task provides a final bounded graceful-shutdown window.
fn request_shutdown(app: &mut App, input_tx: &mpsc::Sender<UserSubmission>) {
    app.should_quit = true;
    app.queue.clear();
    app.active_cancel = None;
    let _ = input_tx.try_send(UserSubmission::Shutdown);
}

async fn open_diff(app: &mut App) {
    match diff::load_worktree().await {
        Ok(document) if document.lines.is_empty() => {
            app.timeline.push(Line::from(Span::styled(
                "  ✓ 工作区没有已跟踪文件变更",
                Style::default().fg(Color::Green),
            )));
        }
        Ok(document) => {
            app.timeline.push(Line::from(vec![
                Span::styled("  Δ ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    document.summary(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " · 完整 diff 已按需打开",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            app.diff_scroll = 0;
            app.diff = Some(document);
        }
        Err(error) => app.timeline.push(Line::from(Span::styled(
            format!("  ⚠ 无法读取 diff: {error}"),
            Style::default().fg(Color::Red),
        ))),
    }
}

async fn handle_ui_event(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ev: UiEvent,
) -> Result<()> {
    match ev {
        UiEvent::TurnStart { cancel_token } => {
            app.in_progress = true;
            app.active_cancel = Some(cancel_token);
        }
        UiEvent::UserEcho(text) => {
            app.used_tokens = app
                .used_tokens
                .saturating_add(estimate_text_tokens(&text) + 4);
            // 把用户消息以 ▸ 前缀插入到 scrollback
            emit_lines(
                app,
                terminal,
                vec![Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Cyan)),
                    Span::raw(text),
                ])],
            )?;
        }
        UiEvent::SystemLine(text) => {
            // 多行系统输出，先剥 ANSI（slash 命令文本可能含历史遗留的颜色码）
            let cleaned = strip_ansi(&text);
            let lines: Vec<Line> = cleaned
                .split('\n')
                .map(|l| {
                    Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ))
                })
                .collect();
            emit_lines(app, terminal, lines)?;
        }
        UiEvent::Thinking => {
            commit_live_region(app, terminal)?;
            // streaming_buf 清空，inline 区会显示一个占位
            app.streaming_buf.clear();
            app.streaming_frozen.clear();
            app.streaming_frozen_tokens = 0;
            app.streaming_finalized = false;
        }
        UiEvent::AssistantDelta(d) => {
            app.streaming_buf.push_str(&d);
            if app.mode == TuiMode::Fullscreen {
                freeze_stable_markdown(app);
            }
        }
        UiEvent::AssistantEnd => {
            app.streaming_finalized = true;
        }
        UiEvent::StreamRetry(reason) => {
            app.streaming_buf.clear();
            app.streaming_frozen.clear();
            app.streaming_frozen_tokens = 0;
            app.streaming_finalized = false;
            emit_lines(
                app,
                terminal,
                vec![Line::from(Span::styled(
                    format!("  ↻ 检测到流式重复，正在重新采样：{reason}"),
                    Style::default().fg(Color::Yellow),
                ))],
            )?;
        }
        UiEvent::ToolCall { name, args } => {
            commit_live_region(app, terminal)?;
            app.used_tokens = app
                .used_tokens
                .saturating_add(estimate_text_tokens(&name) + estimate_text_tokens(&args) + 4);
            emit_lines(
                app,
                terminal,
                vec![
                    Line::from(""),
                    Line::from(vec![Span::styled(
                        format!("🔧 {}", name),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    Line::from(Span::styled(
                        format!("  {}", truncate(&args, 200)),
                        Style::default().fg(Color::DarkGray),
                    )),
                ],
            )?;
        }
        UiEvent::ToolResult { result, is_error } => {
            commit_live_region(app, terminal)?;
            app.used_tokens = app
                .used_tokens
                .saturating_add(estimate_text_tokens(&result) + 4);
            let (icon, color) = if is_error {
                ("⚠️", Color::Red)
            } else {
                ("↳", Color::Green)
            };
            emit_lines(
                app,
                terminal,
                vec![Line::from(vec![
                    Span::styled(format!("{} ", icon), Style::default().fg(color)),
                    Span::raw(truncate(&result, 200)),
                ])],
            )?;
        }
        UiEvent::ToolImage { url, alt } => {
            let artifact = image::ImageArtifact::from_source(url, alt);
            let index = app.images.len();
            let status = if artifact.can_render() {
                format!("{} · 按 i 预览", artifact.protocol.label())
            } else {
                format!(
                    "{} · {}",
                    artifact.protocol.label(),
                    truncate(&artifact.source, 80)
                )
            };
            emit_lines(
                app,
                terminal,
                vec![Line::from(vec![
                    Span::styled("🖼  ", Style::default().fg(Color::Magenta)),
                    Span::styled(
                        artifact.alt.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" · {status}"), Style::default().fg(Color::DarkGray)),
                ])],
            )?;
            app.images.push(artifact);
            if app.mode == TuiMode::Fullscreen && app.images[index].can_render() {
                app.image_preview = Some(index);
                app.image_dirty = true;
            }
        }
        UiEvent::PermissionRequest { name, args, reply } => {
            app.pending_perm = Some(PendingPermission {
                name,
                args,
                reply: Some(reply),
            });
        }
        UiEvent::TurnComplete => {
            commit_live_region(app, terminal)?;
            app.in_progress = false;
            app.streaming_buf.clear();
            app.streaming_frozen.clear();
            app.streaming_frozen_tokens = 0;
            app.streaming_finalized = false;
            app.active_cancel = None;
            // 实际派发由 main loop 的 drain_queue 处理
        }
        UiEvent::UsageUpdate { used, window } => {
            app.used_tokens = used;
            if window > 0 {
                app.window_tokens = window;
            }
        }
        UiEvent::Status(s) => {
            if let Some(m) = s.model {
                app.model = m;
            }
            if let Some(y) = s.yolo {
                app.yolo = y;
            }
            if let Some(mode) = s.permission_mode {
                app.yolo = mode == "full";
                app.permission_mode = mode;
            }
            if let Some(n) = s.session_name {
                app.session_name = n;
            }
            if let Some(p) = s.active_provider {
                // 更新菜单数据源中的 active provider，让 /model 菜单跟着切
                app.sources.active_provider = p;
            }
        }
        UiEvent::Error(msg) => {
            emit_lines(
                app,
                terminal,
                vec![Line::from(Span::styled(
                    format!("❌ {}", msg),
                    Style::default().fg(Color::Red),
                ))],
            )?;
        }
    }
    Ok(())
}

fn emit_lines(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    lines: Vec<Line<'static>>,
) -> Result<()> {
    if app.mode == TuiMode::Fullscreen {
        if app.scroll_from_bottom > 0 {
            app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(lines.len());
        }
        app.timeline.extend(lines);
        Ok(())
    } else {
        insert_lines(terminal, lines)
    }
}

fn can_commit_live_region(finalized: bool, awaiting_user_input: bool) -> bool {
    finalized && !awaiting_user_input
}

fn commit_live_region(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<()> {
    if !can_commit_live_region(app.streaming_finalized, app.pending_perm.is_some()) {
        return Ok(());
    }
    let text = std::mem::take(&mut app.streaming_buf);
    if text.is_empty() && app.streaming_frozen.is_empty() {
        app.streaming_finalized = false;
        return Ok(());
    }
    app.used_tokens = app
        .used_tokens
        .saturating_add(app.streaming_frozen_tokens)
        .saturating_add(estimate_text_tokens(&text) + 4);
    app.streaming_frozen_tokens = 0;
    let mut lines = std::mem::take(&mut app.streaming_frozen);
    if !text.is_empty() {
        lines.extend(
            text.split('\n')
                .map(|line| Line::from(Span::raw(line.to_string()))),
        );
    }
    app.streaming_finalized = false;
    emit_lines(app, terminal, lines)
}

fn stable_markdown_checkpoint(text: &str) -> Option<usize> {
    text.match_indices("\n\n")
        .filter_map(|(index, marker)| {
            let end = index + marker.len();
            let prefix = &text[..end];
            prefix
                .matches("```")
                .count()
                .is_multiple_of(2)
                .then_some(end)
        })
        .last()
}

fn freeze_stable_markdown(app: &mut App) {
    let Some(checkpoint) = stable_markdown_checkpoint(&app.streaming_buf) else {
        return;
    };
    let tail = app.streaming_buf.split_off(checkpoint);
    let stable = std::mem::replace(&mut app.streaming_buf, tail);
    app.streaming_frozen_tokens = app
        .streaming_frozen_tokens
        .saturating_add(estimate_text_tokens(&stable));
    app.streaming_frozen.extend(
        stable
            .split_terminator('\n')
            .map(|line| Line::from(Span::raw(line.to_string()))),
    );
}

/// 把若干行内容插入到 inline viewport 上方（成为 scrollback）。
/// 用 `Buffer::set_line` 直接逐行写入，避开 `Paragraph::wrap` 的 word-wrap
/// ——后者把每个 CJK 字符当独立词，导致每个汉字后塞一个空格。
fn insert_lines(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    lines: Vec<Line<'static>>,
) -> Result<()> {
    let height = lines.len() as u16;
    if height == 0 {
        return Ok(());
    }
    terminal.insert_before(height, |buf: &mut Buffer| {
        let area = buf.area;
        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }
            let y = area.y + i as u16;
            safe_set_line(buf, area.x, y, line, area.width);
        }
    })?;
    Ok(())
}

fn safe_set_line(buf: &mut Buffer, x: u16, y: u16, line: &Line<'_>, width: u16) {
    let area = buf.area;
    if width == 0 || x < area.x || y < area.y || x >= area.right() || y >= area.bottom() {
        return;
    }
    let available = area.right().saturating_sub(x);
    if available == 0 {
        return;
    }
    buf.set_line(x, y, line, width.min(available));
}

// ---- 渲染 ----

fn draw(f: &mut ratatui::Frame, app: &App) {
    if app.mode == TuiMode::Fullscreen {
        draw_fullscreen(f, app);
        return;
    }
    let area = f.area();

    // 自上而下：菜单/流式预览 (5) → 输入框 (3) → 状态栏 (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_top(f, chunks[0], app);
    draw_input(f, chunks[1], app);
    draw_status(f, chunks[2], app);

    if app.pending_perm.is_some() {
        draw_permission_overlay(f, area, app);
    }
    if app.diff.is_some() {
        draw_diff_overlay(f, area, app);
    }
}

fn draw_diff_overlay(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(document) = app.diff.as_ref() else {
        return;
    };
    let modal = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    f.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            format!(" Δ {} · Esc 关闭 ", document.summary()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let inner_height = modal.height.saturating_sub(2) as usize;
    let max_scroll = document.lines.len().saturating_sub(inner_height) as u16;
    f.render_widget(
        Paragraph::new(document.styled_lines())
            .block(block)
            .scroll((app.diff_scroll.min(max_scroll), 0)),
        modal,
    );
}

fn draw_fullscreen(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let input_lines = app.input.split('\n').count().clamp(1, 8) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(input_lines + 2),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, chunks[0], app);
    draw_timeline(f, chunks[1], app);
    draw_input(f, chunks[2], app);
    draw_status(f, chunks[3], app);

    if app.menu.active() {
        let menu_area = fullscreen_menu_area(area, app);
        f.render_widget(Clear, menu_area);
        draw_menu(f, menu_area, app);
    }
    if app.pending_perm.is_some() {
        draw_permission_overlay(f, area, app);
    }
    if app.diff.is_some() {
        draw_diff_overlay(f, area, app);
    }
    if app.image_preview.is_some() {
        draw_image_overlay(f, area, app);
    }
}

fn fullscreen_menu_area(area: Rect, app: &App) -> Rect {
    let height = (app.menu.items.len() as u16 + 2)
        .min(12)
        .min(area.height.saturating_sub(4));
    let width = area.width.saturating_sub(4).min(96);
    let input_height = app.input.split('\n').count().clamp(1, 8) as u16 + 2;
    let input_y = area.bottom().saturating_sub(1).saturating_sub(input_height);
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        input_y.saturating_sub(height),
        width,
        height,
    )
}

fn draw_image_overlay(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(image) = app.image_preview.and_then(|index| app.images.get(index)) else {
        return;
    };
    let modal = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    f.render_widget(Clear, modal);
    let message = if image.can_render() {
        "终端图片预览 · i / Esc 关闭"
    } else {
        "当前终端或图片来源不支持内联预览 · i / Esc 关闭"
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                image.alt.clone(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(message, Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled(
                truncate(&image.source, modal.width.saturating_sub(4) as usize),
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(" 图片 "),
        )
        .alignment(ratatui::layout::Alignment::Center),
        modal,
    );
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let left = format!("  pwcli · {}", app.session_name);
    let right = format!("{}  ", app.model);
    let gap = area
        .width
        .saturating_sub(UnicodeWidthStr::width(left.as_str()) as u16)
        .saturating_sub(UnicodeWidthStr::width(right.as_str()) as u16) as usize;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                left,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, Style::default().fg(Color::DarkGray)),
        ])),
        area,
    );
}

fn draw_timeline(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut lines = app.timeline.clone();
    if !app.streaming_buf.is_empty() || !app.streaming_frozen.is_empty() {
        lines.push(Line::from(""));
        lines.extend(app.streaming_frozen.iter().cloned());
        lines.extend(
            app.streaming_buf
                .split('\n')
                .map(|line| Line::from(Span::raw(line.to_string()))),
        );
    } else if app.in_progress {
        lines.push(Line::from(Span::styled(
            "  💭 思考中…  Ctrl-C 取消",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let visible = area.height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let from_bottom = app.scroll_from_bottom.min(max_scroll);
    let top = max_scroll.saturating_sub(from_bottom) as u16;
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((top, 0)),
        area,
    );
}

fn draw_top(f: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.menu.active() {
        draw_menu(f, area, app);
    } else if !app.streaming_buf.is_empty() {
        draw_streaming(f, area, app);
    } else if app.in_progress {
        // 转 turn 中但没 delta：占位显示"思考中"，给用户实时反馈
        draw_thinking(f, area);
    } else {
        let blank = Paragraph::new("");
        f.render_widget(blank, area);
    }
}

fn draw_thinking(f: &mut ratatui::Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" ⏳ ", Style::default().fg(Color::DarkGray)));
    let body = Paragraph::new(Line::from(Span::styled(
        " 💭 思考中... (Ctrl-C 取消)",
        Style::default().fg(Color::DarkGray),
    )))
    .block(block);
    f.render_widget(body, area);
}

fn draw_menu(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let max_visible = area.height.saturating_sub(2) as usize;
    let total = app.menu.items.len();
    let (start, end) = menu_visible_range(app, max_visible);

    let items: Vec<ListItem> = app.menu.items[start..end]
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let actual_i = start + i;
            let label_style = if actual_i == app.menu.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            let mut spans = vec![Span::styled(format!(" {:<14}", item.label), label_style)];
            if !item.description.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", item.description),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = if app.menu.title.is_empty() {
        format!(" {}/{} ", app.menu.selected + 1, total)
    } else {
        format!(" {} {}/{} ", app.menu.title, app.menu.selected + 1, total)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(title, Style::default().fg(Color::Cyan)));

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn menu_visible_range(app: &App, max_visible: usize) -> (usize, usize) {
    let total = app.menu.items.len();
    let start = if total <= max_visible || app.menu.selected < max_visible / 2 {
        0
    } else {
        (app.menu.selected + 1)
            .saturating_sub(max_visible)
            .min(total.saturating_sub(max_visible))
    };
    (start, (start + max_visible).min(total))
}

fn draw_streaming(f: &mut ratatui::Frame, area: Rect, app: &App) {
    // 仅显示 buf 末尾 N 行
    let body_h = area.height.saturating_sub(2) as usize;
    let lines: Vec<&str> = app.streaming_buf.split('\n').collect();
    let start = lines.len().saturating_sub(body_h);
    let visible: Vec<Line> = lines[start..]
        .iter()
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " 💭 思考中 / 流式输出 ",
            Style::default().fg(Color::DarkGray),
        ));
    let para = Paragraph::new(visible)
        .wrap(Wrap { trim: false })
        .block(block);
    f.render_widget(para, area);
}

fn draw_input(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = app
        .input
        .split('\n')
        .enumerate()
        .map(|(index, text)| {
            let prompt = if index == 0 { "▸ " } else { "  " };
            Line::from(vec![
                Span::styled(
                    prompt,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(text.to_string()),
            ])
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from("▸ "));
    }
    let para = Paragraph::new(lines);
    f.render_widget(para, inner);

    // 设置实际光标（"▸ " 前缀占 2 列）
    let (cursor_row, cursor_col) = app.cursor_visual_position();
    let col = inner.x + 2 + cursor_col;
    let row = inner.y + cursor_row.min(inner.height.saturating_sub(1));
    f.set_cursor_position((col.min(inner.x + inner.width.saturating_sub(1)), row));
}

fn draw_status(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let queue_part = if app.queue.is_empty() {
        String::new()
    } else {
        format!(" · ⏳ 队列 {}", app.queue.len())
    };
    let perm_part = match app.permission_mode.as_str() {
        "prompt" => " · ✋ 权限 请求批准".to_string(),
        "full" => " · 🚀 权限 完全访问".to_string(),
        _ => " · 🛡 权限 替我审批".to_string(),
    };
    let progress_part = if app.in_progress {
        " · ▶ running".to_string()
    } else {
        String::new()
    };
    let token_part = if app.window_tokens > 0 {
        let visible_used = app
            .used_tokens
            .saturating_add(estimate_text_tokens(&app.input))
            .saturating_add(estimate_text_tokens(&app.streaming_buf));
        let pct = (visible_used as f64 / app.window_tokens as f64 * 100.0).min(999.0);
        format!(
            " · 上下文 ≈ {}/{} · {:.1}%",
            format_token_amount(visible_used),
            format_token_amount(app.window_tokens),
            pct,
        )
    } else {
        String::new()
    };
    let txt = format!(
        "  {} · {}{}{}{}{}",
        app.session_name, app.model, token_part, queue_part, perm_part, progress_part
    );
    let para = Paragraph::new(Line::from(Span::styled(
        txt,
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(para, area);
}

fn format_token_amount(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        compact_decimal(tokens as f64 / 1_000_000.0, "M")
    } else if tokens >= 1_000 {
        compact_decimal(tokens as f64 / 1_000.0, "K")
    } else {
        tokens.to_string()
    }
}

fn compact_decimal(value: f64, suffix: &str) -> String {
    if value >= 100.0 {
        format!("{value:.0}{suffix}")
    } else {
        let decimal = format!("{value:.1}");
        format!("{}{suffix}", decimal.trim_end_matches(".0"))
    }
}

fn draw_permission_overlay(f: &mut ratatui::Frame, area: Rect, app: &App) {
    if let Some(perm) = app.pending_perm.as_ref() {
        let w = area.width.saturating_sub(4).min(80);
        let h = 6u16.min(area.height.saturating_sub(2));
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let modal = Rect::new(x, y, w, h);
        f.render_widget(Clear, modal);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Span::styled(
                " ⚠️ 权限确认 (y/n) ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(modal);
        f.render_widget(block, modal);
        let body = vec![
            Line::from(vec![
                Span::styled("工具: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    perm.name.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                truncate(&perm.args, (w as usize).saturating_sub(4)),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  按 y 允许 / n 拒绝 / Esc 取消",
                Style::default().fg(Color::Cyan),
            )),
        ];
        let para = Paragraph::new(body).wrap(Wrap { trim: false });
        f.render_widget(para, inner);
    }
}

/// 剥离 ANSI 转义码 (\x1b[...m / \x1b[...K 等)。
/// slash 命令历史上为 stdout REPL 输出，会带颜色码；TUI 用 ratatui 渲染不解析它们，
/// 直接显示会出现 `[32m...[0m` 这种裸字符串。
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // ESC [ 后吃到第一个字母字符为止
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// 按终端可见列宽截断（CJK 算 2 列，emoji 算 2 列）；超出时尾部追加省略号
fn truncate(s: &str, max_cols: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    let limit = max_cols.saturating_sub(3);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > limit {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::collections::HashMap;

    #[test]
    fn formats_context_token_magnitudes() {
        assert_eq!(format_token_amount(999), "999");
        assert_eq!(format_token_amount(12_400), "12.4K");
        assert_eq!(format_token_amount(1_000_000), "1M");
        assert_eq!(format_token_amount(1_048_576), "1M");
    }

    #[test]
    fn markdown_checkpoint_waits_for_blank_line_outside_fence() {
        assert_eq!(stable_markdown_checkpoint("first\n\nsecond"), Some(7));
        assert_eq!(stable_markdown_checkpoint("```rust\n\ncode"), None);
        assert_eq!(
            stable_markdown_checkpoint("```rust\n\ncode\n```\n\nafter"),
            Some(19)
        );
    }

    #[test]
    fn commit_frontier_keeps_permission_block_live() {
        assert!(can_commit_live_region(true, false));
        assert!(!can_commit_live_region(true, true));
        assert!(!can_commit_live_region(false, false));
    }

    #[test]
    fn safe_buffer_write_drops_out_of_bounds_frames() {
        let mut buffer = Buffer::empty(Rect::new(2, 3, 4, 2));
        safe_set_line(&mut buffer, 99, 99, &Line::from("ignored"), 20);
        safe_set_line(&mut buffer, 2, 3, &Line::from("ok"), 20);
        assert_eq!(buffer[(2, 3)].symbol(), "o");
        assert_eq!(buffer[(3, 3)].symbol(), "k");
    }

    #[test]
    fn editor_moves_and_deletes_whole_graphemes() {
        let mut app = App::new(
            "model".into(),
            32_000,
            "session".into(),
            MenuSources::default(),
            TuiMode::Fullscreen,
        );
        app.insert_text("a👨‍👩‍👧‍👦e\u{301}");
        app.move_cursor_left();
        assert_eq!(&app.input[app.cursor_byte..], "e\u{301}");
        app.delete_char_before();
        assert_eq!(app.input, "ae\u{301}");
    }

    #[test]
    fn fullscreen_renders_header_timeline_and_multiline_prompt() {
        let mut app = App::new(
            "k3".into(),
            1_000_000,
            "demo".into(),
            MenuSources::default(),
            TuiMode::Fullscreen,
        );
        app.timeline.push(Line::from("assistant response"));
        app.input = "first\nsecond".into();
        app.cursor_byte = app.input.len();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("pwcli · demo"));
        assert!(rendered.contains("assistant response"));
        assert!(rendered.contains("first"));
        assert!(rendered.contains("second"));
    }

    #[test]
    fn picker_filters_daemon_client_commands() {
        let sources = MenuSources {
            providers: vec!["kimi".into(), "openai".into()],
            models_by_provider: HashMap::new(),
            active_provider: "kimi".into(),
            sessions: Vec::new(),
        };
        let mut menu = MenuState::default();
        menu.refresh("/di", &sources);
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label, "/diff");
        menu.refresh("/acp-p", &sources);
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].replacement, "/acp-permissions");
    }

    #[test]
    fn slash_exit_and_quit_are_tui_exit_commands() {
        assert!(is_exit_command("/exit"));
        assert!(is_exit_command(" /QUIT "));
        assert!(!is_exit_command("exit"));
        assert!(!is_exit_command("/exit now"));
    }

    #[tokio::test]
    async fn shutdown_disconnects_without_cancelling_daemon_turn() {
        let mut app = App::new(
            "model".into(),
            32_000,
            "session".into(),
            MenuSources::default(),
            TuiMode::Fullscreen,
        );
        let token = CancellationToken::new();
        app.active_cancel = Some(token.clone());
        app.in_progress = true;
        app.queue.push_back("must not run".into());
        let (tx, mut rx) = mpsc::channel(1);

        request_shutdown(&mut app, &tx);

        assert!(app.should_quit);
        assert!(!token.is_cancelled());
        assert!(app.active_cancel.is_none());
        assert!(app.queue.is_empty());
        assert!(matches!(rx.recv().await, Some(UserSubmission::Shutdown)));
    }
}
