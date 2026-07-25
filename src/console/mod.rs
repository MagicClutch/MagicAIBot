//! Asynchronous local terminal input and command parsing.

pub mod commands;

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use crate::{
    console::commands::{ConsoleInput, parse_input},
    error::AppError,
};

pub async fn read_input(
    sender: mpsc::Sender<Result<ConsoleInput, AppError>>,
    shutdown: CancellationToken,
) {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            result = lines.next_line() => match result {
                Ok(Some(line)) => {
                    if sender.send(parse_input(&line)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    let _ = sender.send(Err(AppError::ConsoleInputFailure(error.to_string()))).await;
                    return;
                }
            }
        }
    }
}
