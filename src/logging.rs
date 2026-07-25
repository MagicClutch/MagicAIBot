use std::fmt::Display;

pub fn info(message: impl Display) {
    println!("[INFO] {message}");
}
pub fn success(message: impl Display) {
    println!("[SUCCESS] {message}");
}
pub fn warning(message: impl Display) {
    println!("[WARNING] {message}");
}
pub fn chat_incoming(sender: &str, message: &str) {
    println!("[CHAT] {sender}: {message}");
}
pub fn chat_outgoing(username: &str, message: &str) {
    println!("[CHAT] <{username}> {message}");
}
