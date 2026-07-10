use eframe::egui;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(PartialEq, Clone, Copy, Debug)]
enum VimMode {
    Normal,
    Insert,
    Command,
    Search,
}

struct VimEngine {
    mode: VimMode,
    cursor: usize,
    pending_d: bool,
    pending_g: bool,
    pending_r: bool,
    command_buf: String,
    status_msg: String,
    status_time: Option<std::time::Instant>,
    search_buf: String,
    last_search: String,
}

impl VimEngine {
    fn new() -> Self {
        Self {
            mode: VimMode::Normal,
            cursor: 0,
            pending_d: false,
            pending_g: false,
            pending_r: false,
            command_buf: String::new(),
            status_msg: String::new(),
            status_time: None,
            search_buf: String::new(),
            last_search: String::new(),
        }
    }

    fn clamp(&self, text: &str) -> usize {
        self.cursor.min(text.len())
    }

    fn set_status(&mut self, msg: String) {
        self.status_msg = msg;
        self.status_time = Some(std::time::Instant::now());
    }

    fn line_start(&self, text: &str) -> usize {
        let c = self.clamp(text);
        text[..c].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, text: &str) -> usize {
        let c = self.clamp(text);
        text[c..].find('\n').map(|i| c + i).unwrap_or(text.len())
    }

    fn move_up(&mut self, text: &str) {
        let c = self.clamp(text);
        let col = c - self.line_start(text);
        let prev_end = self.line_start(text).saturating_sub(1);
        if prev_end == 0 && self.line_start(text) == 0 {
            return;
        }
        let prev_start = text[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prev_len = prev_end - prev_start;
        self.cursor = prev_start + col.min(prev_len);
    }

    fn move_down(&mut self, text: &str) {
        let c = self.clamp(text);
        let col = c - self.line_start(text);
        let end = self.line_end(text);
        if end == text.len() {
            return;
        }
        let next_start = end + 1;
        let next_end = text[next_start..]
            .find('\n')
            .map(|i| next_start + i)
            .unwrap_or(text.len());
        let next_len = next_end - next_start;
        self.cursor = next_start + col.min(next_len);
    }

    fn word_forward(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let len = bytes.len();
        let mut i = self.clamp(text);
        if i >= len {
            return;
        }

        let class = |b: u8| -> u8 {
            let ch = b as char;
            if ch.is_whitespace() {
                0
            } else if ch.is_alphanumeric() || ch == '_' {
                1
            } else {
                2
            }
        };

        let start_class = class(bytes[i]);
        if start_class != 0 {
            while i < len && class(bytes[i]) == start_class {
                i += 1;
            }
        }
        while i < len && class(bytes[i]) == 0 {
            i += 1;
        }

        self.cursor = i;
    }

    fn word_back(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = self.clamp(text);
        if i == 0 {
            return;
        }

        let class = |b: u8| -> u8 {
            let ch = b as char;
            if ch.is_whitespace() {
                0
            } else if ch.is_alphanumeric() || ch == '_' {
                1
            } else {
                2
            }
        };

        i -= 1;
        while i > 0 && class(bytes[i]) == 0 {
            i -= 1;
        }
        if i > 0 {
            let run_class = class(bytes[i]);
            while i > 0 && class(bytes[i - 1]) == run_class {
                i -= 1;
            }
        }

        self.cursor = i;
    }

    fn search_next(&mut self, text: &str) {
        if self.last_search.is_empty() {
            return;
        }
        let start = self.clamp(text);
        let from = (start + 1).min(text.len());
        if let Some(pos) = text[from..].find(&self.last_search) {
            self.cursor = from + pos;
            self.set_status(format!("/{}", self.last_search));
        } else if let Some(pos) = text.find(&self.last_search) {
            self.cursor = pos;
            self.set_status(format!("/{} (wrapped)", self.last_search));
        } else {
            self.set_status(format!("Pattern not found: {}", self.last_search));
        }
    }

    fn search_prev(&mut self, text: &str) {
        if self.last_search.is_empty() {
            return;
        }
        let start = self.clamp(text);
        if let Some(pos) = text[..start].rfind(&self.last_search) {
            self.cursor = pos;
            self.set_status(format!("?{}", self.last_search));
        } else if let Some(pos) = text.rfind(&self.last_search) {
            self.cursor = pos;
            self.set_status(format!("?{} (wrapped)", self.last_search));
        } else {
            self.set_status(format!("Pattern not found: {}", self.last_search));
        }
    }

    fn page_down(&mut self, text: &str, lines: usize) {
        for _ in 0..lines {
            self.move_down(text);
        }
    }

    fn page_up(&mut self, text: &str, lines: usize) {
        for _ in 0..lines {
            self.move_up(text);
        }
    }

    fn handle_normal_key(&mut self, key: egui::Key, text: &mut String) -> bool {
        let was_pending_d = self.pending_d;
        self.pending_d = false;
        if key != egui::Key::G {
            self.pending_g = false;
        }

        match key {
            egui::Key::R => {
                self.pending_r = true;
            }
            egui::Key::H => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            egui::Key::L => {
                let c = self.clamp(text);
                let end = self.line_end(text);
                if c < end {
                    self.cursor += 1;
                }
            }
            egui::Key::K => self.move_up(text),
            egui::Key::J => self.move_down(text),
            egui::Key::W => self.word_forward(text),
            egui::Key::B => self.word_back(text),

            egui::Key::Num0 => {
                self.cursor = self.line_start(text);
            }
            egui::Key::Num4 => {
                self.cursor = self.line_end(text);
            }

            egui::Key::X => {
                let c = self.clamp(text);
                if c < text.len() && text.as_bytes()[c] != b'\n' {
                    text.remove(c);
                    self.cursor = self.clamp(text);
                }
            }

            egui::Key::D => {
                if was_pending_d {
                    let start = self.line_start(text);
                    let end = self.line_end(text);
                    if end < text.len() {
                        text.drain(start..=end);
                    } else if start > 0 {
                        text.drain(start - 1..end);
                    } else {
                        text.drain(start..end);
                    }
                    self.cursor = start.min(text.len());
                } else {
                    self.pending_d = true;
                }
            }

            egui::Key::I => {
                self.mode = VimMode::Insert;
            }
            egui::Key::A => {
                let c = self.clamp(text);
                let end = self.line_end(text);
                if c < end {
                    self.cursor += 1;
                }
                self.mode = VimMode::Insert;
            }
            egui::Key::O => {
                let end = self.line_end(text);
                text.insert(end, '\n');
                self.cursor = end + 1;
                self.mode = VimMode::Insert;
            }

            egui::Key::G => {
                if self.pending_g {
                    self.cursor = 0;
                    self.pending_g = false;
                } else {
                    self.pending_g = true;
                }
            }
            egui::Key::Semicolon => {}
            _ => return false,
        }
        true
    }

    fn handle_char_normal(&mut self, c: char, text: &mut String) {
        if self.pending_r {
            self.pending_r = false;
            if !c.is_control() {
                let pos = self.clamp(text);
                if pos < text.len() {
                    let cur_ch = text[pos..].chars().next();
                    if let Some(old) = cur_ch {
                        if old != '\n' {
                            let end = pos + old.len_utf8();
                            text.replace_range(pos..end, &c.to_string());
                        }
                    }
                }
            }
            return;
        }

        match c {
            '$' => {
                self.cursor = self.line_end(text);
            }
            'A' => {
                self.cursor = self.line_end(text);
                self.mode = VimMode::Insert;
            }
            'G' => {
                self.cursor = text.len();
                self.pending_g = false;
            }
            ':' => {
                self.command_buf.clear();
                self.status_msg.clear();
                self.mode = VimMode::Command;
            }
            '/' => {
                self.search_buf.clear();
                self.status_msg.clear();
                self.mode = VimMode::Search;
            }
            _ => {}
        }
    }
}

struct EditorApp {
    text: String,
    file_path: Option<PathBuf>,
    dirty: bool,
    vim_enabled: bool,
    vim: VimEngine,
    show_help: bool,
    last_cursor: usize,
    scroll_offset: f32,
    text_h: f32,
    pending_open: bool,
    pending_save: bool,
    pending_save_as: bool,
    prev_mode: VimMode,
    show_line_numbers: bool,
    gutter_cache: Option<(usize, usize, egui::text::LayoutJob)>,
    syntax_highlighting: bool,
    highlight_cache: Option<(u64, usize, u32, bool, egui::text::LayoutJob)>,
}

impl Default for EditorApp {
    fn default() -> Self {
        Self {
            text: String::new(),
            file_path: None,
            dirty: false,
            vim_enabled: true,
            vim: VimEngine::new(),
            show_help: false,
            last_cursor: usize::MAX,
            scroll_offset: 0.0,
            text_h: 0.0,
            pending_open: false,
            pending_save: false,
            pending_save_as: false,
            prev_mode: VimMode::Normal,
            show_line_numbers: true,
            gutter_cache: None,
            syntax_highlighting: false,
            highlight_cache: None,
        }
    }
}

impl EditorApp {
    fn from_path(path: Option<PathBuf>) -> Self {
        match path {
            Some(p) => match fs::read_to_string(&p) {
                Ok(content) => Self {
                    text: content,
                    file_path: Some(p),
                    show_help: false,
                    ..Default::default()
                },
                Err(_) => Self {
                    file_path: Some(p),
                    show_help: false,
                    ..Default::default()
                },
            },
            None => Self::default(),
        }
    }

    fn open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            if let Ok(content) = fs::read_to_string(&path) {
                self.text = content;
                self.file_path = Some(path);
                self.dirty = false;
                self.vim.cursor = 0;
            }
        }
    }

    fn save_file(&mut self) {
        if let Some(path) = &self.file_path {
            if fs::write(path, &self.text).is_ok() {
                self.dirty = false;
            }
        } else {
            self.save_as();
        }
    }

    fn save_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new().save_file() {
            if fs::write(&path, &self.text).is_ok() {
                self.file_path = Some(path);
                self.dirty = false;
            }
        }
    }

    fn execute_command(&mut self) {
        let cmd = self.vim.command_buf.trim().to_string();
        match cmd.as_str() {
            "w" => {
                if self.file_path.is_some() {
                    self.save_file();
                    let name = self
                        .file_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    self.vim.set_status(format!("\"{}\" written", name));
                } else {
                    self.pending_save_as = true;
                }
            }
            "w!" => {
                if self.file_path.is_some() {
                    self.save_file();
                    self.vim.set_status("written (forced)".to_string());
                } else {
                    self.pending_save_as = true;
                }
            }
            "q" => {
                std::process::exit(0);
            }
            "q!" => {
                std::process::exit(0);
            }
            "wq" | "x" => {
                self.save_file();
                std::process::exit(0);
            }
            other if other.starts_with("w ") => {
                let path = PathBuf::from(other[2..].trim());
                match fs::write(&path, &self.text) {
                    Ok(_) => {
                        self.vim
                            .set_status(format!("\"{}\" written", path.display()));
                        self.file_path = Some(path);
                        self.dirty = false;
                    }
                    Err(e) => {
                        self.vim.set_status(format!("Error: {}", e));
                    }
                }
            }
            "help" | "h" => {
                self.show_help = true;
            }
            _ if !cmd.is_empty() && cmd.chars().all(|c| c.is_ascii_digit()) => {
                let target: usize = cmd.parse().unwrap_or(1);
                self.goto_line(target);
            }
            _ => {
                self.vim.set_status(format!("Not a command: {}", cmd));
            }
        }
        self.vim.command_buf.clear();
        self.vim.mode = VimMode::Normal;
    }

    fn gutter_job(&mut self, total_lines: usize, current_line: usize) -> egui::text::LayoutJob {
        if let Some((t, c, job)) = &self.gutter_cache {
            if *t == total_lines && *c == current_line {
                return job.clone();
            }
        }
        let job = build_gutter_job(total_lines, current_line);
        self.gutter_cache = Some((total_lines, current_line, job.clone()));
        job
    }

    fn highlight_job(
        &mut self,
        cursor_byte: usize,
        text_color: egui::Color32,
        wrap_width: f32,
    ) -> egui::text::LayoutJob {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        let text_hash = hasher.finish();
        let width_key = wrap_width as u32;

        if let Some((h, cur, w, hl, job)) = &self.highlight_cache {
            if *h == text_hash
                && *cur == cursor_byte
                && *w == width_key
                && *hl == self.syntax_highlighting
            {
                return job.clone();
            }
        }

        let job = build_highlight_job(
            &self.text,
            Some(cursor_byte),
            text_color,
            wrap_width,
            self.syntax_highlighting,
        );
        self.highlight_cache = Some((
            text_hash,
            cursor_byte,
            width_key,
            self.syntax_highlighting,
            job.clone(),
        ));
        job
    }

    fn goto_line(&mut self, line_1based: usize) {
        let total = self.text.chars().filter(|&ch| ch == '\n').count() + 1;
        let target = line_1based.clamp(1, total);
        let mut byte = 0usize;
        let mut line = 1usize;
        if target > 1 {
            for (i, ch) in self.text.char_indices() {
                if ch == '\n' {
                    line += 1;
                    if line == target {
                        byte = i + 1;
                        break;
                    }
                }
            }
        }
        self.vim.cursor = byte.min(self.text.len());
        self.vim.set_status(format!("line {}", target));
    }

    fn process_vim_command(&mut self, ctx: &egui::Context) {
        let mut chars: Vec<char> = vec![];
        let mut enter = false;
        let mut esc = false;

        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Text(t) => {
                        for c in t.chars() {
                            chars.push(c);
                        }
                    }
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        enter = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => {
                        esc = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        ..
                    } => {
                        self.vim.command_buf.pop();
                    }
                    _ => {}
                }
            }
        });

        for c in chars {
            self.vim.command_buf.push(c);
        }

        if enter {
            self.execute_command();
        } else if esc {
            self.vim.command_buf.clear();
            self.vim.status_msg.clear();
            self.vim.mode = VimMode::Normal;
        }
    }

    fn process_vim_search(&mut self, ctx: &egui::Context) {
        let mut chars: Vec<char> = vec![];
        let mut enter = false;
        let mut esc = false;

        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Text(t) => {
                        for c in t.chars() {
                            chars.push(c);
                        }
                    }
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        enter = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => {
                        esc = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        ..
                    } => {
                        self.vim.search_buf.pop();
                    }
                    _ => {}
                }
            }
        });

        for c in chars {
            self.vim.search_buf.push(c);
        }

        if enter {
            self.vim.last_search = self.vim.search_buf.clone();
            let text = self.text.clone();
            self.vim.search_next(&text);
            self.vim.mode = VimMode::Normal;
        } else if esc {
            self.vim.search_buf.clear();
            self.vim.status_msg.clear();
            self.vim.mode = VimMode::Normal;
        }
    }

    fn process_vim_normal(&mut self, ctx: &egui::Context) {
        let mut keys: Vec<(egui::Key, egui::Modifiers)> = vec![];
        let mut chars: Vec<char> = vec![];

        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        keys.push((*key, *modifiers));
                    }
                    egui::Event::Text(t) => {
                        for c in t.chars() {
                            chars.push(c);
                        }
                    }
                    _ => {}
                }
            }
        });

        for (key, modifiers) in keys {
            if self.vim.pending_r {
                if key == egui::Key::Escape {
                    self.vim.pending_r = false;
                }
                continue;
            }
            match key {
                egui::Key::Escape => {
                    self.vim.mode = VimMode::Normal;
                }
                egui::Key::Semicolon if modifiers.shift => {
                    self.vim.command_buf.clear();
                    self.vim.status_msg.clear();
                    self.vim.mode = VimMode::Command;
                }
                egui::Key::F if modifiers.ctrl => {
                    self.vim.page_down(&self.text.clone(), 20);
                }
                egui::Key::B if modifiers.ctrl => {
                    self.vim.page_up(&self.text.clone(), 20);
                }
                egui::Key::N => {
                    let text = self.text.clone();
                    if modifiers.shift {
                        self.vim.search_prev(&text);
                    } else {
                        self.vim.search_next(&text);
                    }
                }
                other => {
                    self.vim.handle_normal_key(other, &mut self.text);
                }
            }
        }

        for c in chars {
            if c == ':' {
                self.vim.command_buf.clear();
                self.vim.status_msg.clear();
                self.vim.mode = VimMode::Command;
            } else {
                self.vim.handle_char_normal(c, &mut self.text);
            }
        }
    }
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

const TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "String",
    "u8", "u16", "u32", "u64", "u128", "usize", "Vec", "Option", "Result", "Box", "Rc", "Arc",
    "HashMap", "HashSet",
];

#[derive(Clone, Copy)]
enum Token<'a> {
    Keyword(&'a str),
    Type_(&'a str),
    Comment(&'a str),
    StringLit(&'a str),
    Number(&'a str),
    Normal(&'a str),
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < text.len() {
        if bytes[i] == b'/' && i + 1 < text.len() && bytes[i + 1] == b'*' {
            let mut j = i + 2;
            while j + 1 < text.len() {
                if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    j += 2;
                    break;
                }
                j += 1;
            }
            if j + 1 >= text.len() {
                j = text.len();
            }
            tokens.push(Token::Comment(&text[i..j]));
            i = j;
            continue;
        }

        if bytes[i] == b'/' && i + 1 < text.len() && bytes[i + 1] == b'/' {
            let end = text[i..].find('\n').map(|n| i + n).unwrap_or(text.len());
            tokens.push(Token::Comment(&text[i..end]));
            i = end;
            continue;
        }

        if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < text.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'"' {
                    j += 1;
                    break;
                }
                j += 1;
            }
            tokens.push(Token::StringLit(&text[i..j]));
            i = j;
            continue;
        }

        if bytes[i].is_ascii_digit() {
            let end = text[i..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                .map(|n| i + n)
                .unwrap_or(text.len());
            tokens.push(Token::Number(&text[i..end]));
            i = end;
            continue;
        }

        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let end = text[i..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|n| i + n)
                .unwrap_or(text.len());
            let word = &text[i..end];
            if KEYWORDS.contains(&word) {
                tokens.push(Token::Keyword(word));
            } else if TYPES.contains(&word) {
                tokens.push(Token::Type_(word));
            } else {
                tokens.push(Token::Normal(word));
            }
            i = end;
            continue;
        }

        let char_end = text[i..]
            .char_indices()
            .nth(1)
            .map(|(n, _)| i + n)
            .unwrap_or(text.len());
        tokens.push(Token::Normal(&text[i..char_end]));
        i = char_end;
    }

    tokens
}

fn build_gutter_job(total_lines: usize, current_line: usize) -> egui::text::LayoutJob {
    let mono = egui::FontId::monospace(14.0);
    let dim = egui::Color32::from_rgb(110, 110, 120);
    let bright = egui::Color32::from_rgb(200, 200, 120);

    let width = total_lines.to_string().len().max(3);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;

    for line in 0..total_lines {
        let color = if line == current_line { bright } else { dim };
        let text = format!("{:>width$}\n", line + 1, width = width);
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: mono.clone(),
                color,
                ..Default::default()
            },
        );
    }

    job
}

fn build_highlight_job(
    text: &str,
    cursor_byte: Option<usize>,
    text_color: egui::Color32,
    wrap_width: f32,
    highlight: bool,
) -> egui::text::LayoutJob {
    let mono = egui::FontId::monospace(14.0);
    let kw = egui::Color32::from_rgb(204, 120, 180);
    let ty = egui::Color32::from_rgb(86, 182, 194);
    let comm = egui::Color32::from_rgb(106, 153, 85);
    let string = egui::Color32::from_rgb(152, 195, 121);
    let number = egui::Color32::from_rgb(209, 154, 84);
    let cursor_bg = egui::Color32::from_rgb(130, 200, 130);

    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;

    let fmt = |color: egui::Color32| egui::TextFormat {
        font_id: mono.clone(),
        color,
        ..Default::default()
    };

    let mut spans: Vec<(usize, &str, egui::Color32)> = Vec::new();
    if highlight {
        let tokens = tokenize(text);
        let mut pos = 0usize;
        for token in &tokens {
            let (s, color) = match token {
                Token::Keyword(s) => (*s, kw),
                Token::Type_(s) => (*s, ty),
                Token::Comment(s) => (*s, comm),
                Token::StringLit(s) => (*s, string),
                Token::Number(s) => (*s, number),
                Token::Normal(s) => (*s, text_color),
            };
            spans.push((pos, s, color));
            pos += s.len();
        }
    } else {
        spans.push((0, text, text_color));
    }

    match cursor_byte {
        None => {
            for (_, s, color) in spans {
                job.append(s, 0.0, fmt(color));
            }
        }
        Some(cursor) => {
            let mut cursor_drawn = false;
            for (span_start, s, color) in spans {
                let span_end = span_start + s.len();

                if cursor >= span_end || cursor < span_start {
                    job.append(s, 0.0, fmt(color));
                } else {
                    cursor_drawn = true;
                    let rel = cursor - span_start;
                    let before = &s[..rel];
                    let raw_ch = s[rel..].chars().next();

                    if !before.is_empty() {
                        job.append(before, 0.0, fmt(color));
                    }

                    match raw_ch {
                        None | Some('\n') => {
                            job.append(
                                " ",
                                0.0,
                                egui::TextFormat {
                                    font_id: mono.clone(),
                                    color: egui::Color32::BLACK,
                                    background: cursor_bg,
                                    ..Default::default()
                                },
                            );
                            let after_start = rel + raw_ch.map(|c| c.len_utf8()).unwrap_or(0);
                            if after_start < s.len() {
                                job.append(&s[rel..], 0.0, fmt(color));
                            } else if raw_ch == Some('\n') {
                                job.append("\n", 0.0, fmt(color));
                            }
                        }
                        Some(ch) => {
                            let ch_end = rel + ch.len_utf8();
                            job.append(
                                &s[rel..ch_end],
                                0.0,
                                egui::TextFormat {
                                    font_id: mono.clone(),
                                    color: egui::Color32::BLACK,
                                    background: cursor_bg,
                                    ..Default::default()
                                },
                            );
                            if ch_end < s.len() {
                                job.append(&s[ch_end..], 0.0, fmt(color));
                            }
                        }
                    }
                }
            }

            if !cursor_drawn {
                job.append(
                    " ",
                    0.0,
                    egui::TextFormat {
                        font_id: mono.clone(),
                        color: egui::Color32::BLACK,
                        background: cursor_bg,
                        ..Default::default()
                    },
                );
            }
        }
    }

    job
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let entering_insert_now =
            self.vim.mode == VimMode::Insert && self.prev_mode != VimMode::Insert;
        self.prev_mode = self.vim.mode;

        ctx.input(|i| {
            let cmd_or_ctrl = i.modifiers.ctrl || i.modifiers.command;
            if cmd_or_ctrl && i.key_pressed(egui::Key::O) {
                self.pending_open = true;
            }
            if cmd_or_ctrl && i.key_pressed(egui::Key::S) {
                self.pending_save = true;
            }
        });

        if self.vim_enabled && self.vim.mode == VimMode::Insert {
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if esc {
                self.vim.mode = VimMode::Normal;
            }
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open (Cmd/Ctrl+O)").clicked() {
                    self.pending_open = true;
                }
                if ui.button("Save (Cmd/Ctrl+S)").clicked() {
                    self.pending_save = true;
                }
                if ui.button("Save As").clicked() {
                    self.pending_save_as = true;
                }

                ui.separator();

                if ui.button("Help (:help)").clicked() {
                    self.show_help = true;
                }

                ui.separator();

                if ui.checkbox(&mut self.vim_enabled, "Vim mode").changed() {
                    self.vim.mode = VimMode::Normal;
                    if self.vim_enabled {
                        self.last_cursor = self.vim.cursor;
                    } else {
                        let te_id = ui.make_persistent_id("main_text_edit");
                        let char_idx = self.text[..self.vim.cursor.min(self.text.len())]
                            .chars()
                            .count();
                        let mut state =
                            egui::TextEdit::load_state(ui.ctx(), te_id).unwrap_or_default();
                        let mut ccursor = egui::text::CCursor::new(char_idx);
                        ccursor.prefer_next_row = false;
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
                        egui::TextEdit::store_state(ui.ctx(), te_id, state);
                    }
                }

                ui.checkbox(&mut self.show_line_numbers, "Line numbers");
                ui.checkbox(&mut self.syntax_highlighting, "Syntax");

                if self.vim_enabled {
                    let label = match self.vim.mode {
                        VimMode::Normal => egui::RichText::new("NORMAL")
                            .color(egui::Color32::from_rgb(100, 200, 100))
                            .strong(),
                        VimMode::Insert => egui::RichText::new("INSERT")
                            .color(egui::Color32::from_rgb(100, 150, 255))
                            .strong(),
                        VimMode::Command => egui::RichText::new("COMMAND")
                            .color(egui::Color32::from_rgb(220, 180, 80))
                            .strong(),
                        VimMode::Search => egui::RichText::new("SEARCH")
                            .color(egui::Color32::from_rgb(200, 140, 220))
                            .strong(),
                    };
                    ui.label(label);
                }

                ui.separator();

                let name = self
                    .file_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Untitled");
                ui.label(format!("{}{}", name, if self.dirty { " *" } else { "" }));
            });
        });

        if self.vim_enabled {
            egui::TopBottomPanel::bottom("vim_status").show(ctx, |ui| match self.vim.mode {
                VimMode::Command => {
                    ui.label(egui::RichText::new(format!(":{}", self.vim.command_buf)).monospace());
                }
                VimMode::Search => {
                    ui.label(egui::RichText::new(format!("/{}", self.vim.search_buf)).monospace());
                }
                VimMode::Normal => {
                    let show_status = !self.vim.status_msg.is_empty()
                        && self
                            .vim
                            .status_time
                            .map(|t| t.elapsed().as_millis() < 2000)
                            .unwrap_or(false);

                    if show_status {
                        ui.label(egui::RichText::new(&self.vim.status_msg).size(18.0));
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(250));
                    } else {
                        ui.label(
                            egui::RichText::new(
                                "move: (j/k/l/h) word: (w/b) insert: (i) append: (a) end-line (A) \
                                 new-line: (o) del-char: (x) replace: (r) del-line: (dd) start/end: (0/$)\n\
                                 page-down: (Ctrl+F) page-up: (Ctrl+B) end-file: (G) top-file: (gg) \
                                 /search n/N goto-line: (:n) save: (:w) save-quit: (:wq) quit: (:q)",
                            )
                            .size(18.0)
                            .weak(),
                        );
                    }
                }
                VimMode::Insert => {
                    ui.label(
                        egui::RichText::new("Insert Mode: Esc for Normal Mode")
                            .size(18.0)
                            .weak(),
                    );
                }
            });
        }

        if self.show_help {
            egui::Window::new("Vim Command Reference")
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let section = |ui: &mut egui::Ui, title: &str| {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(title)
                                .strong()
                                .color(egui::Color32::from_rgb(100, 200, 100)),
                        );
                        ui.separator();
                    };

                    let row = |ui: &mut egui::Ui, key: &str, desc: &str| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:10}", key))
                                    .monospace()
                                    .color(egui::Color32::from_rgb(220, 180, 80)),
                            );
                            ui.label(desc);
                        });
                    };

                    egui::ScrollArea::vertical()
                        .max_height(480.0)
                        .show(ui, |ui| {
                            section(ui, "Movement");
                            row(ui, "h", "Move left");
                            row(ui, "l", "Move right");
                            row(ui, "j", "Move down");
                            row(ui, "k", "Move up");
                            row(ui, "w", "Jump to start of next word");
                            row(ui, "b", "Jump to start of previous word");
                            row(ui, "0", "Jump to start of line");
                            row(ui, "$", "Jump to end of line");
                            row(ui, "Ctrl+f", "Page down (20 lines)");
                            row(ui, "Ctrl+b", "Page up (20 lines)");
                            row(ui, "G", "Jump to end of file (Shift+G)");
                            row(ui, "gg", "Jump to top of file (press g twice)");

                            section(ui, "Entering Insert Mode");
                            row(ui, "i", "Insert before cursor");
                            row(ui, "a", "Insert after cursor (append)");
                            row(ui, "A", "Insert at end of line");
                            row(ui, "o", "Open new line below and insert");
                            row(ui, "Esc", "Return to Normal mode");

                            section(ui, "Editing");
                            row(ui, "x", "Delete character under cursor");
                            row(ui, "r<char>", "Replace character under cursor");
                            row(ui, "dd", "Delete current line");

                            section(ui, "Search  (Normal mode)");
                            row(ui, "/text", "Search forward for text, Enter to jump");
                            row(ui, "n", "Jump to next match");
                            row(ui, "N", "Jump to previous match");

                            section(ui, "File Commands  (Normal mode, type : first)");
                            row(ui, ":w", "Save file");
                            row(ui, ":w file", "Save to a new filename");
                            row(ui, ":wq  :x", "Save and quit");
                            row(ui, ":q", "Quit");
                            row(ui, ":q!", "Quit without saving");
                            row(ui, ":help :h", "Show this help window");
                            row(ui, ":<n>", "Jump to line number n");

                            section(ui, "Tips");
                            ui.label("Vim has two main modes: Normal and Insert.");
                            ui.label("You always start in Normal mode.");
                            ui.label("Press i to start typing, Esc to stop.");
                            ui.label("Use hjkl to move — arrow keys also work.");
                            ui.label("Type : to enter a command, Enter to run it.");
                        });

                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        self.show_help = false;
                    }
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.vim_enabled
                && (self.vim.mode == VimMode::Normal
                    || self.vim.mode == VimMode::Command
                    || self.vim.mode == VimMode::Search)
            {
                if self.vim.mode == VimMode::Normal {
                    self.process_vim_normal(ctx);
                } else if self.vim.mode == VimMode::Command {
                    self.process_vim_command(ctx);
                } else if self.vim.mode == VimMode::Search {
                    self.process_vim_search(ctx);
                }

                let c = self.vim.cursor.min(self.text.len());

                let cursor_line = self.text[..c].chars().filter(|&ch| ch == '\n').count();
                let total_lines = self.text.chars().filter(|&ch| ch == '\n').count() + 1;

                let text_color = ui.visuals().text_color();
                let wrap_width = ui.available_width();
                let gutter_job = if self.show_line_numbers {
                    Some(self.gutter_job(total_lines, cursor_line))
                } else {
                    None
                };
                let job = self.highlight_job(c, text_color, wrap_width);

                let cursor_moved = self.vim.cursor != self.last_cursor;
                self.last_cursor = self.vim.cursor;

                let viewport_h = ui.available_height();

                let line_height = if self.text_h > 0.0 && total_lines > 0 {
                    self.text_h / total_lines as f32
                } else {
                    ui.text_style_height(&egui::TextStyle::Monospace)
                };
                let cursor_y = cursor_line as f32 * line_height;

                if cursor_moved {
                    let max_offset = (self.text_h - viewport_h + line_height).max(0.0);
                    let cursor_center = cursor_y + line_height / 2.0;
                    let offset = (cursor_center - viewport_h / 2.0).clamp(0.0, max_offset);
                    self.scroll_offset = offset;
                }

                let out = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset)
                    .show(ui, |ui: &mut egui::Ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            if let Some(gutter) = gutter_job {
                                ui.label(gutter);
                                ui.separator();
                            }
                            let text_rect = ui.label(job).rect;
                            self.text_h = text_rect.height();
                        });
                        ui.add_space(line_height);
                    });

                if !cursor_moved {
                    self.scroll_offset = out.state.offset.y;
                }
            } else {
                let te_id = ui.make_persistent_id("main_text_edit");

                let just_entered_insert = entering_insert_now;

                if just_entered_insert {
                    let char_idx = self.text[..self.vim.cursor.min(self.text.len())]
                        .chars()
                        .count();
                    let mut state = egui::TextEdit::load_state(ui.ctx(), te_id).unwrap_or_default();
                    let mut ccursor = egui::text::CCursor::new(char_idx);
                    ccursor.prefer_next_row = false;
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
                    egui::TextEdit::store_state(ui.ctx(), te_id, state);
                }

                let size = ui.available_size();
                let text_color = ui.visuals().text_color();
                let wrap_w = size.x;
                let hl = self.syntax_highlighting;
                let mut layouter = move |ui: &egui::Ui, s: &str, _wrap: f32| {
                    ui.fonts(|f| f.layout_job(build_highlight_job(s, None, text_color, wrap_w, hl)))
                };

                let gutter_job = if self.show_line_numbers {
                    let total_lines = self.text.chars().filter(|&ch| ch == '\n').count() + 1;
                    let cur = self.vim.cursor.min(self.text.len());
                    let current_line = self.text[..cur].chars().filter(|&ch| ch == '\n').count();
                    Some(self.gutter_job(total_lines, current_line))
                } else {
                    None
                };

                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("editor_scroll")
                    .auto_shrink([false, false]);
                if just_entered_insert {
                    scroll = scroll.vertical_scroll_offset(self.scroll_offset);
                }
                let scroll_out = scroll.show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    ui.horizontal_top(|ui| {
                        if let Some(gutter) = gutter_job {
                            ui.label(gutter);
                            ui.separator();
                        }
                        ui.add(
                            egui::TextEdit::multiline(&mut self.text)
                                .id(te_id)
                                .font(egui::TextStyle::Monospace)
                                .layouter(&mut layouter)
                                .desired_width(f32::INFINITY)
                                .frame(false),
                        )
                    })
                    .inner
                });
                let output = scroll_out.inner;
                if !just_entered_insert {
                    self.scroll_offset = scroll_out.state.offset.y;
                }

                if output.changed() {
                    self.dirty = true;
                }

                if !just_entered_insert {
                    if let Some(state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                        if let Some(range) = state.cursor.char_range() {
                            self.vim.cursor = self
                                .text
                                .char_indices()
                                .nth(range.primary.index)
                                .map(|(i, _)| i)
                                .unwrap_or(self.text.len());
                        }
                    }
                }

                if self.vim_enabled && self.vim.mode == VimMode::Insert {
                    output.request_focus();
                }
            }
        });

        if self.pending_open {
            self.pending_open = false;
            self.open_file();
        }
        if self.pending_save {
            self.pending_save = false;
            self.save_file();
        }
        if self.pending_save_as {
            self.pending_save_as = false;
            self.save_as();
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();
    let path = args.get(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Text Editor",
        options,
        Box::new(|_cc| Ok(Box::new(EditorApp::from_path(path)))),
    )
}
