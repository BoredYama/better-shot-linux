//! BetterShot for Linux.

mod app;
mod capture;
mod cli;
mod clipboard;
mod color_picker;
mod countdown;
mod deck_ui;
mod editor;
mod gallery;
mod onboarding_ui;
mod pin;
mod release_notes_ui;
mod scroll_session;
mod selector;
mod services;
mod settings;
mod sharing;
mod transfer_card;

use adw::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    // Help, version, and typos are answered here, without starting or
    // contacting the running instance.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Help) => {
            print!("{}", cli::help());
            return glib::ExitCode::SUCCESS;
        }
        Ok(cli::Command::Version) => {
            println!("BetterShot {}", bettershot_core::version::APP_VERSION.version);
            return glib::ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            return glib::ExitCode::FAILURE;
        }
        Ok(_) => {}
    }

    let application = adw::Application::builder()
        .application_id(bettershot_core::paths::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    application.connect_startup(app::startup);
    application.connect_command_line(app::handle_command_line);
    application.run()
}
