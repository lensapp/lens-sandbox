//! The conversions that have to match `parse-dockerfile`'s `#[non_exhaustive]` enums. A variant
//! upstream adds later is a case no test can construct, so the wildcard arms that answer for one
//! live here alone.

use parse_dockerfile::{Command as Upstream, Instruction};

/// Where an instruction's keyword begins, which is the line a refusal names.
pub(super) fn keyword_start(instruction: &Instruction<'_>) -> usize {
    match instruction {
        Instruction::Add(i) => i.add.span.start,
        Instruction::Arg(i) => i.arg.span.start,
        Instruction::Cmd(i) => i.cmd.span.start,
        Instruction::Copy(i) => i.copy.span.start,
        Instruction::Entrypoint(i) => i.entrypoint.span.start,
        Instruction::Env(i) => i.env.span.start,
        Instruction::Expose(i) => i.expose.span.start,
        Instruction::From(i) => i.from.span.start,
        Instruction::Healthcheck(i) => i.healthcheck.span.start,
        Instruction::Label(i) => i.label.span.start,
        Instruction::Maintainer(i) => i.maintainer.span.start,
        Instruction::Onbuild(i) => i.onbuild.span.start,
        Instruction::Run(i) => i.run.span.start,
        Instruction::Shell(i) => i.shell.span.start,
        Instruction::Stopsignal(i) => i.stopsignal.span.start,
        Instruction::User(i) => i.user.span.start,
        Instruction::Volume(i) => i.volume.span.start,
        Instruction::Workdir(i) => i.workdir.span.start,
        _ => 0,
    }
}

/// The keyword an instruction outside the v1 subset was written with, so the refusal names it.
pub(super) fn unsupported_keyword(instruction: &Instruction<'_>) -> &'static str {
    match instruction {
        Instruction::Healthcheck(_) => "HEALTHCHECK",
        Instruction::Maintainer(_) => "MAINTAINER",
        Instruction::Onbuild(_) => "ONBUILD",
        Instruction::Stopsignal(_) => "STOPSIGNAL",
        _ => "this instruction",
    }
}

pub(super) fn command(arguments: &Upstream<'_>) -> super::Command {
    match arguments {
        Upstream::Exec(exec) => super::Command::Exec(
            exec.value
                .iter()
                .map(|word| word.value.to_string())
                .collect(),
        ),
        Upstream::Shell(shell) => super::Command::Shell(shell.value.trim().to_string()),
        _ => super::Command::Shell(String::new()),
    }
}
