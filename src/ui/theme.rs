use crate::config::StatusColor;
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    colored: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colored: std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

impl Theme {
    #[cfg(test)]
    pub fn plain() -> Self {
        Self { colored: false }
    }

    fn paint(self, style: Style) -> Style {
        if self.colored {
            style
        } else {
            Style::default()
        }
    }

    pub fn border(self) -> Style {
        self.paint(Style::default().fg(Color::DarkGray))
    }

    pub fn title(self) -> Style {
        self.paint(Style::default().fg(Color::Cyan))
            .add_modifier(Modifier::BOLD)
    }

    pub fn columns(self) -> Style {
        self.paint(Style::default().fg(Color::DarkGray))
            .add_modifier(Modifier::BOLD)
    }

    pub fn muted(self) -> Style {
        self.paint(Style::default().fg(Color::DarkGray))
    }

    pub fn queue(self) -> Style {
        self.paint(Style::default().fg(Color::Blue))
    }

    pub fn job(self) -> Style {
        self.paint(Style::default())
    }

    pub fn badge(self) -> Style {
        self.paint(Style::default().fg(Color::Magenta))
    }

    pub fn label(self) -> Style {
        self.paint(Style::default().fg(Color::Cyan))
    }

    pub fn selected(self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn marker(self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn resting(self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    pub fn notice(self) -> Style {
        self.paint(Style::default().fg(Color::Yellow))
    }

    pub fn alert(self) -> Style {
        self.paint(Style::default().fg(Color::Red))
            .add_modifier(Modifier::BOLD)
    }

    pub fn status(self, color: StatusColor) -> Style {
        let color = match color {
            StatusColor::Plain => return self.paint(Style::default()),
            StatusColor::Indexed(index) => Color::Indexed(index),
            StatusColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
        };
        self.paint(Style::default().fg(color))
    }
}
