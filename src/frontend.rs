use crate::backend::LauncherResult;
use backtrace::Backtrace;
use crossterm::{
    cursor,
    event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use std::time::Duration;
use std::{
    error::Error,
    io::{self, Stdout},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};

// TODO: use stateful list
pub struct App {
    running: bool,
    terminal: Terminal<CrosstermBackend<Stdout>>,
    query: String,
    prompt: String,
    cursor_index: usize,
    list_len: usize,
    list_state: ListState,
    completion: bool,
    completion_content: Option<String>,
}

impl App {
    pub fn init(prompt: &str) -> Result<App, io::Error> {
        std::panic::set_hook(Box::new(move |x| {
            cleanup_terminal();
            let bt = Backtrace::new();
            println!("{:?}", bt);
            print!("{:?}", x);
        }));

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(App {
            running: true,
            terminal,
            query: String::new(),
            prompt: String::from(prompt),
            cursor_index: 0,
            list_len: 0,
            list_state: ListState::default(),
            completion: false,
            completion_content: None,
        })
    }

    pub fn update<'a>(
        &'a mut self,
        list: &'a [LauncherResult],
    ) -> Result<&'a mut App, io::Error> {
        let list = if self.query.is_empty() { &[] } else { list };
        self.list_len = list.len();
        self.fix_selection();
        let mut completion_content = None;
        self.terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [Constraint::Length(3), Constraint::Min(0)].as_ref(),
                )
                .split(f.size());
            // input field
            let block = Block::default().borders(Borders::ALL);
            completion_content = if self.completion {
                Some(
                    list[self.list_state.selected().unwrap()]
                        .get_string()
                        .split_once('|')
                        .unwrap()
                        .1
                        .trim()
                        .to_string(),
                )
            } else {
                None
            };
            let input_field = self.prompt.clone()
                + &completion_content
                    .clone()
                    .unwrap_or_else(|| self.query.clone());
            let len = input_field
                .chars()
                .fold(0, |acc, x| acc + 1 + (x.len_utf8() > 1) as usize);
            let input_field = Text::from(Span::from(input_field));
            let paragraph = Paragraph::new(input_field).block(block);
            f.render_widget(paragraph, chunks[0]);
            f.set_cursor(1 + len as u16, 1);

            // search result
            let items = list
                .iter()
                .map(|r| ListItem::new(Span::from(r.get_string())))
                .collect::<Vec<ListItem>>();
            let items = List::new(items)
                .block(Block::default().borders(Borders::ALL))
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");
            f.render_stateful_widget(items, chunks[1], &mut self.list_state);
        })?;
        self.completion_content = completion_content;
        Ok(self)
    }

    fn replace_query(&mut self) {
        if let Some(s) = &self.completion_content {
            self.query = s.to_string();
            self.cursor_index = self.query.chars().count();
            self.completion = false;
        }
    }

    pub fn wait_input(
        &mut self,
        index: &mut Option<usize>,
    ) -> Result<bool, Box<dyn Error>> {
        loop {
            if !poll(Duration::from_millis(30))? {
                return Ok(false);
            }
            if let Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                state: _,
            }) = read()?
            {
                if code == KeyCode::Char('c')
                    && modifiers.contains(KeyModifiers::CONTROL)
                {
                    return Ok(true);
                }
                macro_rules! move_selection {
                    ($list_len:expr, $state:expr, $i:expr, $dir:expr) => {
                        if $list_len > 0 {
                            $state.select(if let Some(i) = $state.selected() {
                                let i = i as i64 + $dir;
                                let i = if i < 0 {
                                    $list_len - 1
                                } else {
                                    i as usize % $list_len
                                };
                                Some(i)
                            } else {
                                None
                            })
                        }
                    };
                }
                match code {
                    KeyCode::Char(ch) => {
                        self.replace_query();
                        if self.cursor_index == self.query.len() {
                            self.query.push(ch);
                        } else {
                            // insert Char into Chars
                            self.query = self
                                .query
                                .chars()
                                .take(self.cursor_index)
                                .collect::<String>()
                                + &ch.to_string()
                                + &self
                                    .query
                                    .chars()
                                    .skip(self.cursor_index)
                                    .collect::<String>();
                        }
                        self.cursor_index += 1;
                        return Ok(false);
                    }
                    KeyCode::Backspace | KeyCode::Delete => {
                        self.completion = false;
                        if self.cursor_index > 0 {
                            self.query = self
                                .query
                                .chars()
                                .take(self.cursor_index - 1)
                                .chain(
                                    self.query.chars().skip(self.cursor_index),
                                )
                                .collect();
                            self.cursor_index -= 1;
                        }
                        return Ok(false);
                    }
                    KeyCode::Up => {
                        move_selection!(self.list_len, self.list_state, i, -1);
                        return Ok(false);
                    }
                    KeyCode::Down => {
                        move_selection!(self.list_len, self.list_state, i, 1);
                        return Ok(false);
                    }
                    KeyCode::Left => {
                        self.replace_query();
                        self.cursor_index -= (self.cursor_index > 0) as usize;
                        return Ok(false);
                    }
                    KeyCode::Right => {
                        self.replace_query();
                        self.cursor_index +=
                            (self.cursor_index < self.query.len()) as usize;
                        return Ok(false);
                    }
                    KeyCode::Enter => {
                        *index = self.list_state.selected();
                        return Ok(!index.is_none());
                    }
                    KeyCode::Tab => {
                        self.completion = self.list_len > 0;
                        move_selection!(self.list_len, self.list_state, i, 1);
                        return Ok(false);
                    }
                    KeyCode::Esc => {
                        // cancel completion
                        self.completion = false;
                    }
                    _ => return Ok(false),
                }
            }
        }
    }

    pub fn exit(&mut self) {
        if self.running {
            disable_raw_mode().unwrap();
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen,)
                .unwrap();
            self.terminal.show_cursor().unwrap();
            self.running = false
        }
    }

    pub fn get_query(&self) -> String {
        return self.query.clone();
    }

    pub fn set_prompt(&mut self, prompt: &str) -> &mut App {
        self.prompt = prompt.to_string();
        self
    }

    fn fix_selection(&mut self) {
        if self.list_len > 0 {
            match self.list_state.selected() {
                Some(i) => {
                    if i >= self.list_len {
                        self.list_state.select(Some(self.list_len - 1));
                    }
                }
                None => self.list_state.select(Some(0)),
            }
        } else {
            self.list_state.select(None);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.exit()
    }
}

fn cleanup_terminal() {
    let mut stdout = io::stdout();

    // Needed for when ytop is run in a TTY since TTYs don't actually have an alternate screen.
    // Must be executed before attempting to leave the alternate screen so that it only modifies the
    // 		primary screen if we are running in a TTY.
    // If not running in a TTY, then we just end up modifying the alternate screen which should have
    // 		no effect.
    execute!(stdout, cursor::MoveTo(0, 0)).unwrap();
    execute!(stdout, Clear(ClearType::All)).unwrap();

    execute!(stdout, LeaveAlternateScreen).unwrap();
    execute!(stdout, cursor::Show).unwrap();

    disable_raw_mode().unwrap();
}
