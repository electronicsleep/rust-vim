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
    /// `v`: character-wise selection
    Visual,
    /// `V`: line-wise selection
    VisualLine,
}

struct VimEngine {
    mode: VimMode,
    cursor: usize,
    pending_d: bool,
    pending_g: bool,
    pending_r: bool,
    pending_y: bool,
    /// Line-wise yank register (always ends with '\n').
    register: Option<String>,
    /// Set when the register changes, so the app can copy it to the system clipboard.
    register_changed: bool,
    /// true if the register holds whole lines (yy, dd, V), false for a
    /// character-wise selection (v). Decides how p/P paste it.
    register_linewise: bool,
    /// Where the selection started when v/V was pressed. The selection runs
    /// from here to the cursor.
    visual_anchor: usize,
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
            pending_y: false,
            register: None,
            register_changed: false,
            register_linewise: true,
            visual_anchor: 0,
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

    /// Copy the current line (with trailing newline) into the register.
    fn yank_line(&mut self, text: &str) {
        let start = self.line_start(text);
        let end = self.line_end(text);
        let mut line = text[start..end].to_string();
        line.push('\n');
        self.register = Some(line);
        self.register_linewise = true;
        self.register_changed = true;
    }

    fn is_visual(&self) -> bool {
        matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
    }

    /// Byte range [start, end) covered by the current selection.
    /// For V (line-wise) it spans whole lines, not including the final '\n'.
    fn selection_range(&self, text: &str) -> (usize, usize) {
        let a = self.visual_anchor.min(text.len());
        let c = self.clamp(text);
        let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
        if self.mode == VimMode::VisualLine {
            let start = text[..lo].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let end = text[hi..].find('\n').map(|i| hi + i).unwrap_or(text.len());
            (start, end)
        } else {
            // Inclusive of the character under the cursor, like Vim.
            let end = text[hi..]
                .chars()
                .next()
                .map(|ch| hi + ch.len_utf8())
                .unwrap_or(text.len());
            (lo, end)
        }
    }

    /// Copy the selection into the register and move the cursor to its start.
    fn yank_selection(&mut self, text: &str) {
        let (start, end) = self.selection_range(text);
        if self.mode == VimMode::VisualLine {
            let mut t = text[start..end].to_string();
            t.push('\n');
            let n = t.matches('\n').count();
            self.register = Some(t);
            self.register_linewise = true;
            self.set_status(format!("{} line{} yanked", n, if n == 1 { "" } else { "s" }));
        } else {
            let t = text[start..end].to_string();
            let n = t.chars().count();
            self.register = Some(t);
            self.register_linewise = false;
            self.set_status(format!(
                "{} character{} yanked",
                n,
                if n == 1 { "" } else { "s" }
            ));
        }
        self.register_changed = true;
        self.cursor = start;
    }

    /// Cut the selection: copy it into the register, then remove it.
    fn delete_selection(&mut self, text: &mut String) {
        let (start, end) = self.selection_range(text);
        let linewise = self.mode == VimMode::VisualLine;
        self.yank_selection(text);
        if linewise {
            // Same newline handling as dd.
            if end < text.len() {
                text.drain(start..=end);
                self.cursor = start;
            } else if start > 0 {
                text.drain(start - 1..end);
                self.cursor = text[..start - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
            } else {
                text.drain(start..end);
                self.cursor = 0;
            }
        } else {
            text.drain(start..end);
            self.cursor = start.min(text.len());
        }
        self.status_msg = self.status_msg.replace("yanked", "deleted");
    }

    /// Paste the register below (p) or above (P) the current line.
    /// Returns true if anything was pasted.
    fn paste(&mut self, text: &mut String, above: bool) -> bool {
        self.pending_d = false;
        self.pending_g = false;
        self.pending_y = false;
        let Some(reg) = self.register.clone() else {
            self.set_status("Nothing to paste (yank with yy)".to_string());
            return false;
        };
        if !self.register_linewise {
            // Character-wise: P inserts at the cursor, p just after it.
            let c = self.clamp(text);
            let pos = if above {
                c
            } else {
                match text[c..].chars().next() {
                    Some(ch) if ch != '\n' => c + ch.len_utf8(),
                    _ => c,
                }
            };
            text.insert_str(pos, &reg);
            let last = reg.chars().last().map(|ch| ch.len_utf8()).unwrap_or(0);
            self.cursor = pos + reg.len() - last;
            return true;
        }
        if above {
            let start = self.line_start(text);
            text.insert_str(start, &reg);
            self.cursor = start;
        } else {
            let end = self.line_end(text);
            if end == text.len() {
                // Last line has no trailing newline: add one before the pasted line.
                let body = reg.strip_suffix('\n').unwrap_or(&reg);
                text.insert_str(end, &format!("\n{}", body));
            } else {
                text.insert_str(end + 1, &reg);
            }
            self.cursor = end + 1;
        }
        true
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
        let was_pending_y = self.pending_y;
        self.pending_y = false;
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
                    // dd also cuts the line into the register, so dd + p moves a line.
                    self.yank_line(text);
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

            egui::Key::Y => {
                if was_pending_y {
                    self.yank_line(text);
                    self.set_status("1 line yanked (copied to clipboard)".to_string());
                } else {
                    self.pending_y = true;
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

// ---------------------------------------------------------------------------
// File tree (left sidebar)
// ---------------------------------------------------------------------------

struct FileNode {
    path: PathBuf,
    name: String,
    is_dir: bool,
    /// Loaded lazily the first time a directory is expanded.
    children: Option<Vec<FileNode>>,
}

impl FileNode {
    fn new(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.display().to_string());
        let is_dir = path.is_dir();
        Self {
            path,
            name,
            is_dir,
            children: None,
        }
    }

    fn load_children(&mut self) {
        let mut kids: Vec<FileNode> = fs::read_dir(&self.path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    // Skip hidden entries like .git, .DS_Store
                    .filter(|p| {
                        !p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with('.'))
                            .unwrap_or(false)
                    })
                    .map(FileNode::new)
                    .collect()
            })
            .unwrap_or_default();

        // Directories first, then files, both alphabetical (case-insensitive).
        kids.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.children = Some(kids);
    }
}

fn show_file_tree(
    ui: &mut egui::Ui,
    node: &mut FileNode,
    current: Option<&PathBuf>,
    clicked: &mut Option<PathBuf>,
) {
    if node.is_dir {
        egui::CollapsingHeader::new(node.name.clone())
            .id_salt(&node.path)
            .show(ui, |ui| {
                if node.children.is_none() {
                    node.load_children();
                }
                if let Some(children) = node.children.as_mut() {
                    if children.is_empty() {
                        ui.weak("(empty)");
                    }
                    for child in children.iter_mut() {
                        show_file_tree(ui, child, current, clicked);
                    }
                }
            });
    } else {
        let selected = current == Some(&node.path);
        if ui.selectable_label(selected, &node.name).clicked() {
            *clicked = Some(node.path.clone());
        }
    }
}

// ---------------------------------------------------------------------------

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
    pending_open_dir: bool,
    pending_save: bool,
    pending_save_as: bool,
    prev_mode: VimMode,
    show_line_numbers: bool,
    gutter_cache: Option<(usize, usize, egui::text::LayoutJob)>,
    syntax_highlighting: bool,
    highlight_cache: Option<(u64, usize, Option<(usize, usize)>, u32, bool, egui::text::LayoutJob)>,
    undo_stack: Vec<(String, usize)>,
    file_tree: Option<FileNode>,
    show_file_panel: bool,
    /// File the user asked to open while the current buffer had unsaved changes.
    pending_switch: Option<PathBuf>,
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
            pending_open_dir: false,
            pending_save: false,
            pending_save_as: false,
            prev_mode: VimMode::Normal,
            show_line_numbers: true,
            gutter_cache: None,
            syntax_highlighting: false,
            highlight_cache: None,
            undo_stack: Vec::new(),
            file_tree: None,
            show_file_panel: false,
            pending_switch: None,
        }
    }
}

impl EditorApp {
    fn from_path(path: Option<PathBuf>) -> Self {
        let mut app = Self::default();
        if let Some(p) = path {
            if p.is_dir() {
                app.open_dir(p);
            } else {
                if let Ok(content) = fs::read_to_string(&p) {
                    app.text = content;
                }
                app.file_path = Some(p);
            }
        }
        app
    }

    /// Show `dir` in the left sidebar.
    fn open_dir(&mut self, dir: PathBuf) {
        let dir = dir.canonicalize().unwrap_or(dir);
        let mut root = FileNode::new(dir);
        root.load_children();
        self.vim
            .set_status(format!("opened folder {}", root.path.display()));
        self.file_tree = Some(root);
        self.show_file_panel = true;
    }

    /// Load a file into the editor, replacing the current buffer.
    fn load_file(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(content) => {
                self.text = content;
                self.file_path = Some(path);
                self.dirty = false;
                self.vim.cursor = 0;
                self.vim.mode = VimMode::Normal;
                self.undo_stack.clear();
                self.scroll_offset = 0.0;
                self.text_h = 0.0;
                self.last_cursor = usize::MAX;
            }
            Err(e) => {
                self.vim
                    .set_status(format!("Can't open {}: {}", path.display(), e));
            }
        }
    }

    /// Open a file, asking first if the current buffer has unsaved changes.
    fn request_open(&mut self, path: PathBuf) {
        if self.file_path.as_ref() == Some(&path) {
            return;
        }
        if self.dirty {
            self.pending_switch = Some(path);
        } else {
            self.load_file(path);
        }
    }

    fn open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.request_open(path);
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
            // :e <path>  — open a file, or a directory in the sidebar
            other if other.starts_with("e ") || other.starts_with("edit ") => {
                let arg = other.split_once(' ').map(|(_, a)| a.trim()).unwrap_or("");
                let mut path = PathBuf::from(arg);
                if path.is_relative() {
                    if let Some(root) = &self.file_tree {
                        path = root.path.join(path);
                    }
                }
                if path.is_dir() {
                    self.open_dir(path);
                } else if path.is_file() {
                    self.request_open(path);
                } else {
                    self.vim
                        .set_status(format!("No such file or directory: {}", arg));
                }
            }
            // :Ex — toggle the sidebar (opens the current directory if none yet)
            "Ex" | "ex" | "explore" => {
                if self.file_tree.is_some() {
                    self.show_file_panel = !self.show_file_panel;
                } else {
                    let dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    self.open_dir(dir);
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
        selection: Option<(usize, usize)>,
        text_color: egui::Color32,
        wrap_width: f32,
    ) -> egui::text::LayoutJob {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        let text_hash = hasher.finish();
        let width_key = wrap_width as u32;

        if let Some((h, cur, sel, w, hl, job)) = &self.highlight_cache {
            if *h == text_hash
                && *cur == cursor_byte
                && *sel == selection
                && *w == width_key
                && *hl == self.syntax_highlighting
            {
                return job.clone();
            }
        }

        let job = build_highlight_job(
            &self.text,
            Some(cursor_byte),
            selection,
            text_color,
            wrap_width,
            self.syntax_highlighting,
        );
        self.highlight_cache = Some((
            text_hash,
            cursor_byte,
            selection,
            width_key,
            self.syntax_highlighting,
            job.clone(),
        ));
        job
    }

    fn push_undo(&mut self) {
        self.undo_stack.push((self.text.clone(), self.vim.cursor));
        if self.undo_stack.len() > 3 {
            self.undo_stack.remove(0);
        }
    }

    fn undo(&mut self) {
        if let Some((text, cursor)) = self.undo_stack.pop() {
            self.text = text;
            self.vim.cursor = cursor.min(self.text.len());
            self.dirty = true;
            self.vim.set_status("undo".to_string());
        } else {
            self.vim.set_status("Already at oldest change".to_string());
        }
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
        let mut pushed_undo = false;

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

        if self.vim.is_visual() {
            let changed = self.process_visual(&keys, &chars);
            self.finish_vim_frame(ctx, changed);
            return;
        }

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
                egui::Key::U => {
                    self.undo();
                }
                egui::Key::V => {
                    // Start a selection at the cursor.
                    self.vim.visual_anchor = self.vim.cursor.min(self.text.len());
                    self.vim.pending_d = false;
                    self.vim.pending_y = false;
                    self.vim.pending_g = false;
                    self.vim.mode = if modifiers.shift {
                        VimMode::VisualLine
                    } else {
                        VimMode::Visual
                    };
                }
                egui::Key::P => {
                    self.push_undo();
                    pushed_undo = true;
                    let mut text = std::mem::take(&mut self.text);
                    self.vim.paste(&mut text, modifiers.shift);
                    self.text = text;
                }
                other => {
                    let edits = match other {
                        egui::Key::X | egui::Key::O | egui::Key::I | egui::Key::A => true,
                        egui::Key::D => self.vim.pending_d,
                        _ => false,
                    };
                    if edits {
                        self.push_undo();
                        pushed_undo = true;
                    }
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
                if self.vim.pending_r {
                    self.push_undo();
                    pushed_undo = true;
                }
                self.vim.handle_char_normal(c, &mut self.text);
            }
        }

        self.finish_vim_frame(ctx, pushed_undo);
    }

    /// Keys while in Visual / Visual Line mode. Returns true if text may have changed.
    fn process_visual(&mut self, keys: &[(egui::Key, egui::Modifiers)], chars: &[char]) -> bool {
        let mut changed = false;
        for &(key, modifiers) in keys {
            if !self.vim.is_visual() {
                break;
            }
            match key {
                egui::Key::Escape => self.vim.mode = VimMode::Normal,
                egui::Key::V => {
                    // v in v-mode (or V in V-mode) exits; the other one switches kind.
                    let target = if modifiers.shift {
                        VimMode::VisualLine
                    } else {
                        VimMode::Visual
                    };
                    self.vim.mode = if self.vim.mode == target {
                        VimMode::Normal
                    } else {
                        target
                    };
                }
                egui::Key::Y => {
                    self.vim.yank_selection(&self.text);
                    self.vim.mode = VimMode::Normal;
                }
                egui::Key::D | egui::Key::X => {
                    self.push_undo();
                    changed = true;
                    self.vim.delete_selection(&mut self.text);
                    self.vim.mode = VimMode::Normal;
                }
                egui::Key::O => {
                    // Jump to the other end of the selection.
                    let c = self.vim.cursor;
                    self.vim.cursor = self.vim.visual_anchor;
                    self.vim.visual_anchor = c;
                }
                egui::Key::F if modifiers.ctrl => self.vim.page_down(&self.text.clone(), 20),
                egui::Key::B if modifiers.ctrl => self.vim.page_up(&self.text.clone(), 20),
                egui::Key::N => {
                    let text = self.text.clone();
                    if modifiers.shift {
                        self.vim.search_prev(&text);
                    } else {
                        self.vim.search_next(&text);
                    }
                }
                // Movement keys extend the selection.
                egui::Key::H
                | egui::Key::J
                | egui::Key::K
                | egui::Key::L
                | egui::Key::W
                | egui::Key::B
                | egui::Key::Num0
                | egui::Key::Num4
                | egui::Key::G => {
                    self.vim.handle_normal_key(key, &mut self.text);
                }
                _ => {}
            }
        }
        for &c in chars {
            if self.vim.is_visual() && (c == '$' || c == 'G') {
                self.vim.handle_char_normal(c, &mut self.text);
            }
        }
        changed
    }

    /// End-of-frame bookkeeping shared by Normal and Visual mode.
    fn finish_vim_frame(&mut self, ctx: &egui::Context, pushed_undo: bool) {
        // Mirror yanked/cut text to the system clipboard.
        if self.vim.register_changed {
            self.vim.register_changed = false;
            if let Some(reg) = &self.vim.register {
                ctx.copy_text(reg.clone());
            }
        }

        // Mark the buffer dirty if a normal-mode edit actually changed the text,
        // so switching files from the sidebar can warn about unsaved changes.
        if pushed_undo {
            if let Some((before, _)) = self.undo_stack.last() {
                if before != &self.text {
                    self.dirty = true;
                }
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
    selection: Option<(usize, usize)>,
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
    let sel_bg = egui::Color32::from_rgba_unmultiplied(80, 130, 220, 110);

    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;

    let fmt = |color: egui::Color32, selected: bool| egui::TextFormat {
        font_id: mono.clone(),
        color,
        background: if selected {
            sel_bg
        } else {
            egui::Color32::TRANSPARENT
        },
        ..Default::default()
    };
    let cursor_fmt = egui::TextFormat {
        font_id: mono.clone(),
        color: egui::Color32::BLACK,
        background: cursor_bg,
        ..Default::default()
    };

    // 1. Split the text into colored spans (one big span if highlighting is off).
    let mut spans: Vec<(usize, &str, egui::Color32)> = Vec::new();
    if highlight {
        let mut pos = 0usize;
        for token in &tokenize(text) {
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

    // 2. Byte positions where the look changes: selection edges and the
    //    cursor character. Spans get cut at these points.
    let mut cuts: Vec<usize> = Vec::new();
    if let Some((s, e)) = selection {
        cuts.push(s);
        cuts.push(e);
        // Isolate each selected '\n' so it can be drawn as a visible block.
        for (i, _) in text[s..e].match_indices('\n') {
            cuts.push(s + i);
            cuts.push(s + i + 1);
        }
    }
    if let Some(c) = cursor_byte {
        if let Some(ch) = text.get(c..).and_then(|t| t.chars().next()) {
            cuts.push(c);
            cuts.push(c + ch.len_utf8());
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    // 3. Emit each piece with the right background.
    for (span_start, s, color) in spans {
        let span_end = span_start + s.len();
        let mut points = vec![span_start];
        points.extend(cuts.iter().copied().filter(|&p| p > span_start && p < span_end));
        points.push(span_end);

        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a == b {
                continue;
            }
            let piece = &s[a - span_start..b - span_start];
            let selected = selection.map(|(ss, se)| a >= ss && a < se).unwrap_or(false);

            if cursor_byte == Some(a) {
                if piece == "\n" {
                    // Cursor at end of line: draw a block, then the newline.
                    job.append(" ", 0.0, cursor_fmt.clone());
                    job.append(piece, 0.0, fmt(color, selected));
                } else {
                    job.append(piece, 0.0, cursor_fmt.clone());
                }
            } else if selected && piece == "\n" {
                // Show a selected line break (so empty selected lines are visible).
                job.append(" ", 0.0, fmt(color, true));
                job.append(piece, 0.0, fmt(color, false));
            } else {
                job.append(piece, 0.0, fmt(color, selected));
            }
        }
    }

    // Cursor past the last character.
    if cursor_byte.map(|c| c >= text.len()).unwrap_or(false) {
        job.append(" ", 0.0, cursor_fmt);
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
                if ui.button("Open Folder").clicked() {
                    self.pending_open_dir = true;
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
                if self.file_tree.is_some() {
                    ui.checkbox(&mut self.show_file_panel, "Files");
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
                        VimMode::Search => egui::RichText::new("SEARCH")
                            .color(egui::Color32::from_rgb(200, 140, 220))
                            .strong(),
                        VimMode::Visual => egui::RichText::new("VISUAL")
                            .color(egui::Color32::from_rgb(240, 140, 90))
                            .strong(),
                        VimMode::VisualLine => egui::RichText::new("V-LINE")
                            .color(egui::Color32::from_rgb(240, 140, 90))
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
                                "move (j/k/l/h) word (w/b) insert (i) append (a) end-line (A) \
                                 new-line (o) del-char (x) replace (u) del-line (dd) yank-line (yy) paste (p/P) visual (v/V) start/end (0/$)\n\
                                 page-down (Ctrl+F) page-up (Ctrl+B) end-file (G) top-file (gg) \
                                 /search (n/N) goto-line (:n) open (:e path) files (:Ex) \
                                 save (:w) save-quit (:wq) quit (:q)",
                            )
                            .size(18.0)
                            .weak(),
                        );
                    }
                }
                VimMode::Visual | VimMode::VisualLine => {
                    let (s, e) = self.vim.selection_range(&self.text);
                    let info = if self.vim.mode == VimMode::VisualLine {
                        let n = self.text[s..e].matches('\n').count() + 1;
                        format!("{} line{}", n, if n == 1 { "" } else { "s" })
                    } else {
                        let n = self.text[s..e].chars().count();
                        format!("{} char{}", n, if n == 1 { "" } else { "s" })
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{} selected   move to extend   y yank   d delete   \
                             o other end   Esc cancel",
                            info
                        ))
                        .size(18.0)
                        .weak(),
                    );
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
                            row(ui, "u", "Undo");
                            row(ui, "dd", "Delete (cut) current line");
                            row(ui, "yy", "Yank (copy) current line");
                            row(ui, "p", "Paste below current line");
                            row(ui, "P", "Paste above current line");

                            section(ui, "Visual Mode  (select text)");
                            row(ui, "v", "Start selecting characters");
                            row(ui, "V", "Start selecting whole lines");
                            row(ui, "move", "h j k l w b 0 $ gg G extend the selection");
                            row(ui, "y", "Yank (copy) the selection");
                            row(ui, "d  x", "Delete (cut) the selection");
                            row(ui, "o", "Jump to the other end of the selection");
                            row(ui, "Esc  v", "Cancel the selection");
                            row(ui, "p  P", "After v-yank: paste after / at the cursor");

                            section(ui, "Search  (Normal mode)");
                            row(ui, "/text", "Search forward for text, Enter to jump");
                            row(ui, "n", "Jump to next match");
                            row(ui, "N", "Jump to previous match");

                            section(ui, "File Commands  (Normal mode, type : first)");
                            row(ui, ":w", "Save file");
                            row(ui, ":w file", "Save to a new filename");
                            row(ui, ":e path", "Open a file, or a folder in the sidebar");
                            row(ui, ":Ex", "Toggle the file sidebar");
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
                            ui.label("Launch with a folder (editor ./src) to browse it.");
                        });

                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        self.show_help = false;
                    }
                });
        }

        // ---- Unsaved-changes prompt when switching files ----
        if let Some(target) = self.pending_switch.clone() {
            let cur_name = self
                .file_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_string();
            let target_name = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();

            // 0 = save, 1 = discard, 2 = cancel
            let mut choice: Option<u8> = None;
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Save changes to \"{}\" before opening \"{}\"?",
                        cur_name, target_name
                    ));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            choice = Some(0);
                        }
                        if ui.button("Discard").clicked() {
                            choice = Some(1);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(2);
                        }
                    });
                });

            match choice {
                Some(0) => {
                    self.save_file();
                    if !self.dirty {
                        self.pending_switch = None;
                        self.load_file(target);
                    }
                }
                Some(1) => {
                    self.pending_switch = None;
                    self.load_file(target);
                }
                Some(2) => {
                    self.pending_switch = None;
                }
                _ => {}
            }
        }

        // ---- File sidebar (must be added before the CentralPanel) ----
        let mut clicked: Option<PathBuf> = None;
        if self.show_file_panel {
            let current = self.file_path.clone();
            if let Some(root) = self.file_tree.as_mut() {
                let mut refresh = false;
                let mut close = false;
                egui::SidePanel::left("file_panel")
                    .resizable(true)
                    .default_width(220.0)
                    .width_range(120.0..=500.0)
                    .show(ctx, |ui| {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.strong(&root.name)
                                .on_hover_text(root.path.display().to_string());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("x").on_hover_text("Hide").clicked() {
                                        close = true;
                                    }
                                    if ui.small_button("Refresh").clicked() {
                                        refresh = true;
                                    }
                                },
                            );
                        });
                        ui.separator();
                        egui::ScrollArea::both()
                            .id_salt("file_tree_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if let Some(children) = root.children.as_mut() {
                                    if children.is_empty() {
                                        ui.weak("(empty folder)");
                                    }
                                    for child in children.iter_mut() {
                                        show_file_tree(ui, child, current.as_ref(), &mut clicked);
                                    }
                                }
                            });
                    });
                if refresh {
                    root.load_children();
                }
                if close {
                    self.show_file_panel = false;
                }
            }
        }
        if let Some(path) = clicked {
            self.request_open(path);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.vim_enabled
                && (self.vim.mode == VimMode::Normal
                    || self.vim.is_visual()
                    || self.vim.mode == VimMode::Command
                    || self.vim.mode == VimMode::Search)
            {
                if self.vim.mode == VimMode::Normal || self.vim.is_visual() {
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
                let selection = if self.vim.is_visual() {
                    Some(self.vim.selection_range(&self.text))
                } else {
                    None
                };
                let job = self.highlight_job(c, selection, text_color, wrap_width);

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
                    ui.fonts(|f| f.layout_job(build_highlight_job(s, None, None, text_color, wrap_w, hl)))
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
        if self.pending_open_dir {
            self.pending_open_dir = false;
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                self.open_dir(dir);
            }
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
