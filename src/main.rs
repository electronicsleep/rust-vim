use eframe::egui;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(PartialEq, Clone, Copy, Debug)]
enum VimMode {
    Normal,
    Insert,
    Command,
}

struct VimEngine {
    mode: VimMode,
    cursor: usize,
    pending_d: bool,
    pending_g: bool,
    command_buf: String,
    status_msg: String,
}

impl VimEngine {
    fn new() -> Self {
        Self {
            mode: VimMode::Normal,
            cursor: 0,
            pending_d: false,
            pending_g: false,
            command_buf: String::new(),
            status_msg: String::new(),
        }
    }

    fn clamp(&self, text: &str) -> usize {
        self.cursor.min(text.len())
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
        if prev_end == 0 && self.line_start(text) == 0 { return; }
        let prev_start = text[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prev_len = prev_end - prev_start;
        self.cursor = prev_start + col.min(prev_len);
    }

    fn move_down(&mut self, text: &str) {
        let c = self.clamp(text);
        let col = c - self.line_start(text);
        let end = self.line_end(text);
        if end == text.len() { return; }
        let next_start = end + 1;
        let next_end = text[next_start..].find('\n')
            .map(|i| next_start + i)
            .unwrap_or(text.len());
        let next_len = next_end - next_start;
        self.cursor = next_start + col.min(next_len);
    }

    fn word_forward(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = self.clamp(text);
        while i < bytes.len() && (bytes[i] as char).is_alphanumeric() { i += 1; }
        while i < bytes.len() && (bytes[i] as char).is_whitespace() { i += 1; }
        self.cursor = i;
    }

    fn word_back(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = self.clamp(text);
        if i == 0 { return; }
        i -= 1;
        while i > 0 && (bytes[i] as char).is_whitespace() { i -= 1; }
        while i > 0 && (bytes[i - 1] as char).is_alphanumeric() { i -= 1; }
        self.cursor = i;
    }

    fn page_down(&mut self, text: &str, lines: usize) {
        for _ in 0..lines { self.move_down(text); }
    }

    fn page_up(&mut self, text: &str, lines: usize) {
        for _ in 0..lines { self.move_up(text); }
    }

    fn handle_normal_key(&mut self, key: egui::Key, text: &mut String) -> bool {
        let was_pending_d = self.pending_d;
        self.pending_d = false;
        if key != egui::Key::G { self.pending_g = false; }

        match key {
            egui::Key::H => {
                if self.cursor > 0 { self.cursor -= 1; }
            }
            egui::Key::L => {
                let c = self.clamp(text);
                let end = self.line_end(text);
                if c < end { self.cursor += 1; }
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
                if c < end { self.cursor += 1; }
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
            egui::Key::Semicolon => {
            }
            _ => return false,
        }
        true
    }

    fn handle_char_normal(&mut self, c: char, text: &mut String) {
        match c {
            '$' => { self.cursor = self.line_end(text); }
            'A' => { self.cursor = self.line_end(text); self.mode = VimMode::Insert; }
            'G' => { self.cursor = text.len(); self.pending_g = false; }
            ':' => {
                self.command_buf.clear();
                self.status_msg.clear();
                self.mode = VimMode::Command;
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
}

impl Default for EditorApp {
    fn default() -> Self {
        Self {
            text: String::new(),
            file_path: None,
            dirty: false,
            vim_enabled: false,
            vim: VimEngine::new(),
            show_help: false,
            last_cursor: usize::MAX,
            scroll_offset: 0.0,
            text_h: 0.0,
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
                self.save_file();
                let name = self.file_path.as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string();
                self.vim.status_msg = format!("\"{}\" written", name);
            }
            "w!" => {
                self.save_file();
                self.vim.status_msg = "written (forced)".to_string();
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
                        self.vim.status_msg = format!("\"{}\" written", path.display());
                        self.file_path = Some(path);
                        self.dirty = false;
                    }
                    Err(e) => {
                        self.vim.status_msg = format!("Error: {}", e);
                    }
                }
            }
            "help" | "h" => {
                self.show_help = true;
            }
            _ => {
                self.vim.status_msg = format!("Not a command: {}", cmd);
            }
        }
        self.vim.command_buf.clear();
        self.vim.mode = VimMode::Normal;
    }

    fn process_vim_command(&mut self, ctx: &egui::Context) {
        let mut chars: Vec<char> = vec![];
        let mut enter = false;
        let mut esc = false;

        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Text(t) => {
                        for c in t.chars() { chars.push(c); }
                    }
                    egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => {
                        enter = true;
                    }
                    egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                        esc = true;
                    }
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                        self.vim.command_buf.pop();
                    }
                    _ => {}
                }
            }
        });

        for c in chars { self.vim.command_buf.push(c); }

        if enter {
            self.execute_command();
        } else if esc {
            self.vim.command_buf.clear();
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
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        keys.push((*key, *modifiers));
                    }
                    egui::Event::Text(t) => {
                        for c in t.chars() { chars.push(c); }
                    }
                    _ => {}
                }
            }
        });

        for (key, modifiers) in keys {
            match key {
                egui::Key::Escape => { self.vim.mode = VimMode::Normal; }
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
                other => { self.vim.handle_normal_key(other, &mut self.text); }
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
    "as", "async", "await", "break", "const", "continue", "crate", "dyn",
    "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

const TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128",
    "isize", "str", "String", "u8", "u16", "u32", "u64", "u128", "usize",
    "Vec", "Option", "Result", "Box", "Rc", "Arc", "HashMap", "HashSet",
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
        if bytes[i] == b'/' && i + 1 < text.len() && bytes[i + 1] == b'/' {
            let end = text[i..].find('\n').map(|n| i + n).unwrap_or(text.len());
            tokens.push(Token::Comment(&text[i..end]));
            i = end;
            continue;
        }

        if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < text.len() {
                if bytes[j] == b'\\' { j += 2; continue; }
                if bytes[j] == b'"' { j += 1; break; }
                j += 1;
            }
            tokens.push(Token::StringLit(&text[i..j]));
            i = j;
            continue;
        }

        if bytes[i].is_ascii_digit() {
            let end = text[i..].find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                .map(|n| i + n).unwrap_or(text.len());
            tokens.push(Token::Number(&text[i..end]));
            i = end;
            continue;
        }

        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let end = text[i..].find(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|n| i + n).unwrap_or(text.len());
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

        let char_end = text[i..].char_indices().nth(1).map(|(n, _)| i + n).unwrap_or(text.len());
        tokens.push(Token::Normal(&text[i..char_end]));
        i = char_end;
    }

    tokens
}

fn build_highlight_job(
    text: &str,
    cursor_byte: Option<usize>,
    text_color: egui::Color32,
    wrap_width: f32,
) -> egui::text::LayoutJob {
    let mono   = egui::FontId::monospace(14.0);
    let kw     = egui::Color32::from_rgb(204, 120, 180);
    let ty     = egui::Color32::from_rgb(86,  182, 194);
    let comm   = egui::Color32::from_rgb(128, 128, 128);
    let string = egui::Color32::from_rgb(152, 195, 121);
    let number = egui::Color32::from_rgb(209, 154,  84);
    let cursor_bg = egui::Color32::from_rgb(130, 200, 130);

    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;

    let fmt = |color: egui::Color32| egui::TextFormat {
        font_id: mono.clone(),
        color,
        ..Default::default()
    };

    let tokens = tokenize(text);

    let mut spans: Vec<(usize, &str, egui::Color32)> = Vec::new();
    let mut pos = 0usize;
    for token in &tokens {
        let (s, color) = match token {
            Token::Keyword(s)   => (*s, kw),
            Token::Type_(s)     => (*s, ty),
            Token::Comment(s)   => (*s, comm),
            Token::StringLit(s) => (*s, string),
            Token::Number(s)    => (*s, number),
            Token::Normal(s)    => (*s, text_color),
        };
        spans.push((pos, s, color));
        pos += s.len();
    }

    match cursor_byte {
        None => {
            for (_, s, color) in spans {
                job.append(s, 0.0, fmt(color));
            }
        }
        Some(cursor) => {
            for (span_start, s, color) in spans {
                let span_end = span_start + s.len();

                if cursor >= span_end || cursor < span_start {
                    job.append(s, 0.0, fmt(color));
                } else {
                    let rel = cursor - span_start;
                    let before = &s[..rel];
                    let raw_ch = s[rel..].chars().next();

                    if !before.is_empty() {
                        job.append(before, 0.0, fmt(color));
                    }

                    match raw_ch {
                        None | Some('\n') => {
                            job.append(" ", 0.0, egui::TextFormat {
                                font_id: mono.clone(),
                                color: egui::Color32::BLACK,
                                background: cursor_bg,
                                ..Default::default()
                            });
                            let after_start = rel + raw_ch.map(|c| c.len_utf8()).unwrap_or(0);
                            if after_start < s.len() {
                                job.append(&s[rel..], 0.0, fmt(color));
                            } else if raw_ch == Some('\n') {
                                job.append("\n", 0.0, fmt(color));
                            }
                        }
                        Some(ch) => {
                            let ch_end = rel + ch.len_utf8();
                            job.append(&s[rel..ch_end], 0.0, egui::TextFormat {
                                font_id: mono.clone(),
                                color: egui::Color32::BLACK,
                                background: cursor_bg,
                                ..Default::default()
                            });
                            if ch_end < s.len() {
                                job.append(&s[ch_end..], 0.0, fmt(color));
                            }
                        }
                    }
                }
            }
        }
    }

    job
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {

        ctx.input(|i| {
            let cmd_or_ctrl = i.modifiers.ctrl || i.modifiers.command;
            if cmd_or_ctrl && i.key_pressed(egui::Key::O) { self.open_file(); }
            if cmd_or_ctrl && i.key_pressed(egui::Key::S) { self.save_file(); }
        });

        if self.vim_enabled && self.vim.mode == VimMode::Insert {
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if esc {
                self.vim.mode = VimMode::Normal;
            }
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open (Cmd/Ctrl+O)").clicked() { self.open_file(); }
                if ui.button("Save (Cmd/Ctrl+S)").clicked() { self.save_file(); }
                if ui.button("Save As").clicked()       { self.save_as(); }

                ui.separator();

                if ui.button("Help (:help)").clicked() {
                    self.show_help = true;
                }

                ui.separator();

                if ui.checkbox(&mut self.vim_enabled, "Vim mode").changed() {
                    self.vim.mode = VimMode::Normal;
                    if !self.vim_enabled {
                        let te_id = ui.make_persistent_id("main_text_edit");
                        let char_idx = self.text[..self.vim.cursor.min(self.text.len())]
                            .chars().count();
                        let mut state = egui::TextEdit::load_state(ui.ctx(), te_id)
                            .unwrap_or_default();
                        let mut ccursor = egui::text::CCursor::new(char_idx);
                        ccursor.prefer_next_row = false;
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
                        egui::TextEdit::store_state(ui.ctx(), te_id, state);
                    }
                }

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
                    };
                    ui.label(label);
                }

                ui.separator();

                let name = self.file_path.as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Untitled");
                ui.label(format!("{}{}", name, if self.dirty { " *" } else { "" }));
            });
        });

        if self.vim_enabled {
            egui::TopBottomPanel::bottom("vim_status").show(ctx, |ui| {
                match self.vim.mode {
                    VimMode::Command => {
                        ui.label(egui::RichText::new(
                            format!(":{}", self.vim.command_buf)
                        ).monospace());
                    }
                    VimMode::Normal => {
                        if !self.vim.status_msg.is_empty() {
                            ui.label(egui::RichText::new(&self.vim.status_msg).small());
                        } else {
                            ui.label(egui::RichText::new(
                                "h/j/k/l  w/b word  i insert  a append  A eol  \
                                 o new-line  x del-char  dd del-line  0/$ start/end  \
                                 Ctrl+F page-down  Ctrl+B page-up  G end-of-file  gg top-of-file  \
                                 :w save  :wq save+quit  :q quit"
                            ).small().weak());
                        }
                    }
                    VimMode::Insert => {
                        ui.label(egui::RichText::new("Type freely — Esc → Normal mode").small().weak());
                    }
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
                        ui.label(egui::RichText::new(title).strong().color(egui::Color32::from_rgb(100, 200, 100)));
                        ui.separator();
                    };

                    let row = |ui: &mut egui::Ui, key: &str, desc: &str| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{:10}", key)).monospace().color(egui::Color32::from_rgb(220, 180, 80)));
                            ui.label(desc);
                        });
                    };

                    egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
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
                        row(ui, "dd", "Delete current line");

                        section(ui, "File Commands  (Normal mode, type : first)");
                        row(ui, ":w", "Save file");
                        row(ui, ":w file", "Save to a new filename");
                        row(ui, ":wq  :x", "Save and quit");
                        row(ui, ":q", "Quit");
                        row(ui, ":q!", "Quit without saving");
                        row(ui, ":help :h", "Show this help window");

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
            if self.vim_enabled && (self.vim.mode == VimMode::Normal || self.vim.mode == VimMode::Command) {
                if self.vim.mode == VimMode::Normal {
                    self.process_vim_normal(ctx);
                } else if self.vim.mode == VimMode::Command {
                    self.process_vim_command(ctx);
                }

                let c = self.vim.cursor.min(self.text.len());

                let cursor_line = self.text[..c].chars().filter(|&ch| ch == '\n').count();
                let total_lines = self.text.chars().filter(|&ch| ch == '\n').count() + 1;

                let text_color = ui.visuals().text_color();
                let wrap_width = ui.available_width();
                let job = build_highlight_job(&self.text, Some(c), text_color, wrap_width);

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
                    let max_offset = (self.text_h - viewport_h).max(0.0);
                    let cursor_center = cursor_y + line_height / 2.0;
                    let mut offset = cursor_center - viewport_h / 2.0;
                    offset = offset.clamp(0.0, max_offset);
                    self.scroll_offset = offset;
                }

                let out = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(self.scroll_offset)
                    .show(ui, |ui: &mut egui::Ui| {
                        let text_rect = ui.label(job).rect;
                        self.text_h = text_rect.height();
                    });

                if !cursor_moved {
                    self.scroll_offset = out.state.offset.y;
                }

            } else {
                let te_id = ui.make_persistent_id("main_text_edit");

                if self.vim_enabled && self.vim.mode == VimMode::Insert {
                    let char_idx = self.text[..self.vim.cursor.min(self.text.len())]
                        .chars().count();
                    let mut state = egui::TextEdit::load_state(ui.ctx(), te_id)
                        .unwrap_or_default();
                    let mut ccursor = egui::text::CCursor::new(char_idx);
                    ccursor.prefer_next_row = false;
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
                    egui::TextEdit::store_state(ui.ctx(), te_id, state);
                }

                let size = ui.available_size();
                let text_color = ui.visuals().text_color();
                let mut layouter = move |ui: &egui::Ui, s: &str, _wrap: f32| {
                    ui.fonts(|f| f.layout_job(build_highlight_job(s, None, text_color, size.x)))
                };

                let output = egui::ScrollArea::vertical()
                    .id_salt("editor_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut self.text)
                            .id(te_id)
                            .font(egui::TextStyle::Monospace)
                            .layouter(&mut layouter)
                            .desired_width(size.x)
                            .frame(false))
                    }).inner;

                if output.changed() { self.dirty = true; }

                if let Some(state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                    if let Some(range) = state.cursor.char_range() {
                        self.vim.cursor = self.text
                            .char_indices()
                            .nth(range.primary.index)
                            .map(|(i, _)| i)
                            .unwrap_or(self.text.len());
                    }
                }

                if self.vim_enabled && self.vim.mode == VimMode::Insert {
                    output.request_focus();
                }
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();
    let path = args.get(1).map(PathBuf::from);

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Text Editor",
        options,
        Box::new(|_cc| Ok(Box::new(EditorApp::from_path(path)))),
    )
}
