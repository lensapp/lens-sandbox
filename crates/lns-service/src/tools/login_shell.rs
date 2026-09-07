use crate::log;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

/// The file `/etc/profile` sources right after it assigns `PATH`, on Debian and on Alpine alike.
pub const GUEST_PROFILE_SNIPPET_PATH: &str = "/etc/profile.d/lens-tools.sh";

/// What a login shell has to read to keep the run's declared tools: `/etc/profile` assigns `PATH` outright, so every tool dir the workload's own PATH carries goes back in front of it from the file that reset sources next. An image with no `/etc/profile` sources nothing, so it gets no file.
pub fn profile_spec(
    bin_paths: &[String],
    image_reads_etc_profile: bool,
) -> Option<RuntimeFileSpec> {
    if bin_paths.is_empty() {
        return None;
    }
    if !image_reads_etc_profile {
        log::debug!(
            "the image ships no /etc/profile, so nothing would source {GUEST_PROFILE_SNIPPET_PATH}; the declared tools reach the workload on its own PATH only"
        );
        return None;
    }
    Some(RuntimeFileSpec {
        guest_path: GUEST_PROFILE_SNIPPET_PATH.to_string(),
        mode: 0o644,
        source: RuntimeSource::Bytes(profile_snippet(bin_paths).into_bytes()),
    })
}

const DIRS_PLACEHOLDER: &str = "{dirs}";

/// Prepend rather than assign: a login shell that already read the user's own `~/.profile` keeps what it added, a dir the shell has is not repeated, and no blank segment is left behind — to `execvp` a blank one is the current directory.
const SNIPPET: &str = r#"# Written by lns for this run: /etc/profile assigns PATH, which drops the tools this sandbox declared.
lens_tools_prefix=''
for lens_tools_dir in {dirs}; do
    case ":${PATH-}:" in
        *":$lens_tools_dir:"*) ;;
        *) lens_tools_prefix="${lens_tools_prefix:+$lens_tools_prefix:}$lens_tools_dir" ;;
    esac
done
if [ -n "$lens_tools_prefix" ]; then
    PATH="$lens_tools_prefix${PATH:+:$PATH}"
    export PATH
fi
unset lens_tools_prefix lens_tools_dir
"#;

fn profile_snippet(bin_paths: &[String]) -> String {
    let dirs: Vec<String> = bin_paths.iter().map(|dir| shell_word(dir)).collect();
    SNIPPET.replace(DIRS_PLACEHOLDER, &dirs.join(" "))
}

/// One shell word whatever the path holds: the dirs are interpolated into a `for` list, and a name carrying a quote would otherwise be re-read as shell syntax.
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    fn body(spec: &RuntimeFileSpec) -> String {
        String::from_utf8(
            spec.source
                .as_bytes()
                .expect("the snippet travels as an inline body")
                .to_vec(),
        )
        .expect("the snippet is utf-8")
    }

    #[test]
    fn the_snippet_lands_where_etc_profile_sources_it_and_the_workload_can_read_it() {
        // /etc/profile assigns PATH and then sources this directory, so a file anywhere else is never read; a workload that is not root still has to be able to read it.
        let spec = profile_spec(&dirs(&["/.lens/tools/jq/1.7.1/data/installs/jq/1.7.1"]), true)
            .expect("a declared tool stages a snippet");
        assert_eq!(spec.guest_path, "/etc/profile.d/lens-tools.sh");
        assert_eq!(spec.mode, 0o644);
    }

    #[test]
    fn the_snippet_puts_the_tool_dirs_in_front_of_the_path_the_login_shell_has() {
        // The whole body is pinned: it is POSIX shell that runs under Debian's bash and Alpine's ash, it keeps declaration order, it extends the shell's PATH rather than assigning one, it never dereferences an unset PATH (a `set -u` shell would abort the whole login), and it unsets its own two variables so the next profile.d file sees neither.
        let spec = profile_spec(&dirs(&["/t/a/bin", "/t/b/bin"]), true).expect("a snippet");
        assert_eq!(
            body(&spec),
            r#"# Written by lns for this run: /etc/profile assigns PATH, which drops the tools this sandbox declared.
lens_tools_prefix=''
for lens_tools_dir in '/t/a/bin' '/t/b/bin'; do
    case ":${PATH-}:" in
        *":$lens_tools_dir:"*) ;;
        *) lens_tools_prefix="${lens_tools_prefix:+$lens_tools_prefix:}$lens_tools_dir" ;;
    esac
done
if [ -n "$lens_tools_prefix" ]; then
    PATH="$lens_tools_prefix${PATH:+:$PATH}"
    export PATH
fi
unset lens_tools_prefix lens_tools_dir
"#
        );
    }

    #[test]
    fn a_tool_dir_carrying_a_quote_arrives_as_one_literal_word() {
        let spec = profile_spec(&dirs(&["/t/it's/bin"]), true).expect("a snippet");
        assert!(
            body(&spec).contains(r"for lens_tools_dir in '/t/it'\''s/bin'; do"),
            "a dir the shell would re-read as syntax must arrive quoted: {}",
            body(&spec)
        );
    }

    #[test]
    fn a_run_that_declared_no_tool_stages_nothing() {
        // Nothing to put back, and a file whose `for` list is empty is a file the run did not need.
        assert!(profile_spec(&[], true).is_none());
    }

    #[test]
    fn an_image_with_no_etc_profile_stages_nothing_and_says_so_on_the_trace_stream() {
        // Nothing would ever source the file, and a run is not worth failing — or warning — over a file no shell reads.
        let messages = crate::test_env::captured_messages(|| {
            assert!(profile_spec(&dirs(&["/t/a/bin"]), false).is_none());
        });
        assert!(
            messages
                .iter()
                .any(|m| m.contains("no /etc/profile") && m.contains(GUEST_PROFILE_SNIPPET_PATH)),
            "an operator reading the trace must be told which file was skipped and why: {messages:?}"
        );
    }
}
