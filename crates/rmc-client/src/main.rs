mod app;
mod api_client;
mod player;
pub mod views;

use app::RmcApp;

fn main() -> iced::Result {
    iced::application(RmcApp::new, RmcApp::update, RmcApp::view)
        .title("RustMediaCenter")
        .run()
}
