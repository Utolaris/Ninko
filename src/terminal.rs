//! Read terminal key events in raw mode, including both CSI and SS3 arrow keys.
use crossterm::{
    cursor::{Hide, MoveToColumn, MoveUp, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use futures_util::{Stream, StreamExt, stream};
use pinyin::ToPinyin;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Latency {
    Pending,
    Ready(u64),
    Timeout,
}

fn latency_color(delay: Option<u64>) -> u8 {
    match delay {
        None => 1,
        Some(0..200) => 2,
        Some(200..=400) => 4,
        Some(_) => 208,
    }
}

fn node_label(name: &str, latency: Option<Latency>, selected: bool) -> String {
    if selected {
        let suffix = match latency {
            None => String::new(),
            Some(Latency::Pending) => "  测速中…".into(),
            Some(Latency::Ready(ms)) => format!("  {ms} ms"),
            Some(Latency::Timeout) => "  超时".into(),
        };
        return console::style(format!("{name}{suffix}"))
            .color256(183)
            .bold()
            .force_styling(true)
            .to_string();
    }
    let color = match latency {
        Some(Latency::Ready(ms)) => Some(latency_color(Some(ms))),
        Some(Latency::Timeout) => Some(latency_color(None)),
        _ => None,
    };
    let name = if let Some(color) = color {
        console::style(name).color256(color).to_string()
    } else {
        name.to_owned()
    };
    match latency {
        None => name,
        Some(Latency::Pending) => format!("{name}  {}", console::style("测速中…").dim()),
        Some(Latency::Ready(ms)) => format!(
            "{name}  {}",
            console::style(format!("{ms} ms")).color256(latency_color(Some(ms)))
        ),
        Some(Latency::Timeout) => format!(
            "{name}  {}",
            console::style("超时").color256(latency_color(None))
        ),
    }
}

struct Appearance<'a> {
    live: bool,
    footer: Option<&'a str>,
    active: Option<usize>,
}

fn pinyin_key(text: &str) -> String {
    let mut key = String::new();
    for ch in text.chars() {
        if let Some(pinyin) = ch.to_pinyin() {
            key.push_str(pinyin.plain());
        } else {
            key.extend(ch.to_lowercase());
        }
    }
    key
}

fn search_keys(text: &str) -> [String; 3] {
    let mut initials = String::new();
    for ch in text.chars() {
        if let Some(pinyin) = ch.to_pinyin() {
            initials.push_str(pinyin.first_letter());
        } else {
            initials.extend(ch.to_lowercase());
        }
    }
    [text.to_lowercase(), pinyin_key(text), initials]
}

fn matches_query(keys: &[String; 3], query: &str) -> bool {
    let query = query.trim().to_lowercase();
    keys.iter().any(|key| key.contains(&query))
}

fn footer_lines(footer: Option<&str>, width: usize) -> Vec<String> {
    let Some(footer) = footer else {
        return Vec::new();
    };
    let mut lines = vec![String::new(), "  全局配置".into()];
    let mut line = String::from("  ");
    for ch in footer.chars() {
        if console::measure_text_width(&line) + console::measure_text_width(&ch.to_string())
            > width.max(4)
        {
            lines.push(line);
            line = "  ".into();
        }
        line.push(ch);
    }
    lines.push(line);
    lines
}

struct Screen {
    lines: u16,
}

impl Screen {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let screen = Self { lines: 0 };
        execute!(io::stdout(), Hide)?;
        Ok(screen)
    }

    fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        let mut out = io::stdout().lock();
        if self.lines > 0 {
            queue!(out, MoveUp(self.lines))?;
        }
        queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        let width = terminal::size()?.0.saturating_sub(1) as usize;
        for line in lines {
            queue!(
                out,
                Print(console::truncate_str(line, width, "…")),
                Print("\r\n")
            )?;
        }
        self.lines = lines.len() as u16;
        out.flush()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.lines > 0 {
            let _ = execute!(out, MoveUp(self.lines));
        }
        let _ = execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown), Show);
        let _ = terminal::disable_raw_mode();
    }
}

pub async fn select(
    prompt: &str,
    items: &[String],
    default: usize,
    searchable: bool,
    footer: Option<&str>,
) -> io::Result<Option<usize>> {
    select_inner(
        prompt,
        items,
        default,
        searchable,
        Appearance {
            live: false,
            footer,
            active: None,
        },
        stream::empty(),
    )
    .await
}

pub async fn select_live<S: Stream<Item = (usize, Option<u64>)> + Unpin>(
    prompt: &str,
    items: &[String],
    active: Option<usize>,
    delays: S,
) -> io::Result<Option<usize>> {
    select_inner(
        prompt,
        items,
        active.unwrap_or(0),
        true,
        Appearance {
            live: true,
            footer: None,
            active,
        },
        delays,
    )
    .await
}

async fn select_inner<S: Stream<Item = (usize, Option<u64>)> + Unpin>(
    prompt: &str,
    items: &[String],
    default: usize,
    searchable: bool,
    appearance: Appearance<'_>,
    mut delays: S,
) -> io::Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut screen = Screen::enter()?;
    let search_keys: Vec<_> = items.iter().map(|item| search_keys(item)).collect();
    let mut query = String::new();
    let mut selected = default.min(items.len() - 1);
    let mut latencies = vec![appearance.live.then_some(Latency::Pending); items.len()];
    let mut finished = !appearance.live;
    let mut previous = Vec::new();
    loop {
        let matches: Vec<_> = items
            .iter()
            .enumerate()
            .filter_map(|(i, _)| matches_query(&search_keys[i], &query).then_some((i, 0)))
            .collect();
        selected = selected.min(matches.len().saturating_sub(1));
        let (width, height) = terminal::size()?;
        let footer = footer_lines(appearance.footer, width.saturating_sub(1) as usize);
        let visible = usize::from(height)
            .saturating_sub(4 + footer.len())
            .clamp(1, 15);
        let start = selected.saturating_sub(visible - 1);
        let mut lines = vec![format!("  {prompt}")];
        if searchable {
            lines.push(format!("  搜索：{query}"));
        }
        for (position, (index, _)) in matches.iter().enumerate().skip(start).take(visible) {
            let label = node_label(
                &items[*index],
                latencies[*index],
                position == selected || appearance.active == Some(*index),
            );
            lines.push(if position == selected {
                format!(
                    "{} {label}",
                    console::style("❯").color256(183).bold().force_styling(true)
                )
            } else {
                format!("  {label}")
            });
        }
        if matches.is_empty() {
            lines.push("  没有匹配节点，请修改关键词".into());
        }
        lines.extend(footer);
        if lines != previous {
            screen.draw(&lines)?;
            previous = lines;
        }
        tokio::select! {
            result = delays.next(), if !finished => {
                match result {
                    Some((index, delay)) => latencies[index] = Some(delay.map(Latency::Ready).unwrap_or(Latency::Timeout)),
                    None => finished = true,
                }
                continue;
            }
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        if !event::poll(Duration::ZERO)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(None);
        }
        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                selected = if selected == 0 {
                    matches.len().saturating_sub(1)
                } else {
                    selected - 1
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                selected = if matches.is_empty() {
                    0
                } else {
                    (selected + 1) % matches.len()
                }
            }
            KeyCode::Enter if !matches.is_empty() => return Ok(Some(matches[selected].0)),
            KeyCode::Esc => return Ok(None),
            KeyCode::Backspace if searchable => {
                query.pop();
                selected = 0;
            }
            KeyCode::Char(c)
                if searchable
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                query.push(c);
                selected = 0;
            }
            _ => {}
        }
    }
}

pub fn wait_key() -> io::Result<()> {
    show_result(&["  按任意键返回菜单".into()])
}

/// 在切换等临时界面展示结果；按任意键后清除，不带到上层菜单。
pub fn show_result(lines: &[String]) -> io::Result<()> {
    let mut screen = Screen::enter()?;
    let mut draw = lines.to_vec();
    if !draw.iter().any(|line| line.contains("按任意键")) {
        draw.push("  按任意键返回菜单".into());
    }
    screen.draw(&draw)?;
    loop {
        if let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::latency_color;
    #[test]
    fn pinyin_preserves_mixed_node_names() {
        assert_eq!(
            super::pinyin_key("🇭🇰 香港01[CF优选]"),
            "🇭🇰 xianggang01[cfyouxuan]"
        );
        assert_eq!(super::pinyin_key("日本 Tokyo 02"), "riben tokyo 02");
    }
    #[test]
    fn hong_kong_matches_partial_full_and_initials() {
        for name in ["🇭🇰 香港01[D专线]", "高速-香港-CF", "香港二  ← 当前使用"]
        {
            for query in ["香港", "xiang", "xianggang", "xg", "XIANG", "XG"] {
                assert!(
                    super::matches_query(&super::search_keys(name), query),
                    "{name}: {query}"
                );
            }
        }
        assert!(!super::matches_query(
            &super::search_keys("🇯🇵 日本01"),
            "xg"
        ));
    }

    #[test]
    fn selected_entire_row_is_lavender_for_every_status() {
        for latency in [
            None,
            Some(super::Latency::Pending),
            Some(super::Latency::Ready(123)),
            Some(super::Latency::Timeout),
        ] {
            let rendered = super::node_label("节点", latency, true);
            assert!(rendered.contains("38;5;183m"));
            assert!(!rendered.contains("38;5;2m"));
            assert!(!rendered.contains("38;5;1m"));
        }
    }
    #[test]
    fn latency_thresholds() {
        assert_eq!(latency_color(None), 1);
        for ms in [1, 199] {
            assert_eq!(latency_color(Some(ms)), 2);
        }
        for ms in [200, 399, 400] {
            assert_eq!(latency_color(Some(ms)), 4);
        }
        for ms in [401, 5000] {
            assert_eq!(latency_color(Some(ms)), 208);
        }
    }
}
