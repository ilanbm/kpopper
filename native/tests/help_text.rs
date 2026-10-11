//! Every option of these commands explains itself in `--help`.
use clap::{Args, Command};

fn undescribed(command: &Command, path: &str, missing: &mut Vec<String>) {
    for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
        if arg
            .get_help()
            .is_none_or(|help| help.to_string().trim().is_empty())
        {
            missing.push(format!("{path} {}", arg.get_id()));
        }
    }
    for sub in command.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        let path = format!("{path} {}", sub.get_name());
        if sub.get_about().is_none() {
            missing.push(format!("{path} (about)"));
        }
        undescribed(sub, &path, missing);
    }
}

#[test]
fn covered_commands_describe_every_option_and_subcommand() {
    let commands = [
        <kpop_native::public_export::CommandOptions as Args>::augment_args(Command::new("export")),
        <kpop_native::public_history::Options as Args>::augment_args(Command::new("history")),
        <kpop_native::public_amend::AnswerOptions as Args>::augment_args(Command::new("answer")),
        <kpop_native::public_pending::Options as Args>::augment_args(Command::new("pending")),
        <kpop_native::public_board::Options as Args>::augment_args(Command::new("board")),
    ];
    let mut missing = Vec::new();
    for command in &commands {
        undescribed(command, command.get_name(), &mut missing);
    }
    assert!(missing.is_empty(), "no help text: {missing:#?}");
}

#[test]
fn history_has_an_about() {
    let history =
        <kpop_native::public_history::Options as Args>::augment_args(Command::new("history"));
    assert!(
        history
            .get_about()
            .is_some_and(|about| !about.to_string().trim().is_empty()),
        "history has no about line"
    );
}
