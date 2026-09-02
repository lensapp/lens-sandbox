use crate::E2eWorld;
use crate::specutil::{arg_parser::split_args, assert_contains, run_cli_in_dir};
use cucumber::{given, then, when};

const KEPT: &str =
    "apiVersion: lns.run/v1\nkind: sandbox\nname: kept\nspec:\n  image: alpine:3.20\n";

#[given(regex = r#"^a working directory holding "([^"]+)"$"#)]
fn a_working_directory_holding(world: &mut E2eWorld, file: String) {
    let dir = world
        .project
        .get_or_insert_with(|| tempfile::TempDir::new().expect("project tempdir"));
    std::fs::write(dir.path().join(file), KEPT).expect("seed the file the save must not destroy");
}

#[when(regex = r#"^I run "([^"]*)" in that directory$"#)]
fn i_run_in_that_directory(world: &mut E2eWorld, cmd_line: String) {
    let dir = world
        .project
        .as_ref()
        .expect("Given a working directory first")
        .path()
        .to_path_buf();
    let mut envs: Vec<(&str, std::ffi::OsString)> = Vec::new();
    if let Some(home) = &world.home {
        envs.push(("HOME", home.path().into()));
        envs.push(("LNS_HOME", home.path().join(".lns").into()));
    }
    world.result = Some(run_cli_in_dir(&dir, split_args(&cmd_line), envs));
}

#[then(regex = r#"^the command fails with an exit code other than 0$"#)]
fn the_command_fails(world: &mut E2eWorld) -> Result<(), String> {
    let result = world.result.as_ref().ok_or("no CLI run captured")?;
    if result.exit_code == 0 {
        return Err(format!(
            "expected a refusal, got exit 0 with:\n{}\n{}",
            result.stdout, result.stderr
        ));
    }
    Ok(())
}

/// §8.5 has `lns` create no file the user did not name, and the old design created a decisions file here on the first run.
#[then(regex = r#"^the working directory holds only "([^"]+)"$"#)]
fn the_working_directory_holds_only(world: &mut E2eWorld, file: String) -> Result<(), String> {
    let dir = world
        .project
        .as_ref()
        .ok_or("Given a working directory first")?
        .path();
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    found.sort();
    if found == [file.clone()] {
        Ok(())
    } else {
        Err(format!(
            "a run that writes into the project puts state where the user keeps their own files; expected only {file:?}, found {found:?}"
        ))
    }
}

#[then(regex = r#"^"([^"]+)" still holds what it held$"#)]
fn the_file_is_untouched(world: &mut E2eWorld, file: String) -> Result<(), String> {
    let dir = world
        .project
        .as_ref()
        .ok_or("Given a working directory first")?
        .path();
    let found = std::fs::read_to_string(dir.join(file)).map_err(|e| e.to_string())?;
    assert_contains(
        &found,
        KEPT.trim(),
        "the file the save refused to overwrite",
    )
}
