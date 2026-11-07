//! Shell integration: makes the shell report its working directory (OSC 7)
//! at every prompt, without touching the user's rc files — the data behind
//! `GhosttyView::live_cwd` and the "new tab follows the front session" rule.
//!
//! The mechanism per shell (the standard overlay trick — generated startup
//! files that chain the user's own; the same face the Tauri host's terminal
//! feature ships, independent implementation):
//! - **zsh**: a generated `ZDOTDIR` (`.zshenv`/`.zprofile`/`.zshrc`/
//!   `.zlogin`) that replays the user's real startup files around the hook,
//!   so `-l` login treatment and history survive the overlay;
//! - **bash**: `--init-file` with a generated rc that replays the login
//!   sequence (or `~/.bashrc`) and chains `PROMPT_COMMAND`;
//! - fish/nu and unknown shells: no injection — the OSC 7 pwd simply never
//!   arrives and the new-tab cwd falls back to the project root.

use std::path::PathBuf;

/// The shell family a `spawn_program` program name belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShellKind {
    Zsh,
    Bash,
    Other,
}

/// The basename of a shell path decides (`/bin/zsh`, `~/.local/bin/nu`…).
pub fn detect(program: &str) -> ShellKind {
    match std::path::Path::new(program).file_name().and_then(|s| s.to_str()) {
        Some("zsh") => ShellKind::Zsh,
        Some("bash") | Some("bash.exe") => ShellKind::Bash,
        _ => ShellKind::Other,
    }
}

/// One injection's payload: env pairs to set and argv entries to append.
pub struct Injection {
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
}

/// Build the injection for `kind`, or `None` when the shell gets no hook.
/// `args` are the shell's own resolved launch arguments — an explicit
/// invocation (a program or script, not a plain interactive shell) skips the
/// hook, the same rule the spawn path applies to login treatment.
pub fn inject(kind: &ShellKind, args: &[&str]) -> Option<Injection> {
    let interactive = args.iter().all(|a| a.starts_with('-'));
    match kind {
        ShellKind::Zsh if interactive => Some(zsh_injection()),
        ShellKind::Bash if interactive => Some(bash_injection(args)),
        _ => None,
    }
}

/// Directory holding the generated startup files (written on demand; static
/// contents per version).
fn scripts_dir() -> PathBuf {
    std::env::temp_dir().join(format!("openidea-gui-shell-integration-{}", env!("CARGO_PKG_VERSION")))
}

fn write_if_changed(path: &PathBuf, content: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|c| c == content) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)
}

/// zsh: point `ZDOTDIR` at the generated set; each generated file sources
/// the user's real one (located through `OPENIDEA_USER_ZDOTDIR`, resolved by
/// `.zshenv` against the inherited `ZDOTDIR` or `$HOME`) and hands `ZDOTDIR`
/// back — login files keep running in the right order and `HISTFILE`
/// resolves to the user's.
const ZSHENV: &str = r#"_openidea_dir=$ZDOTDIR
ZDOTDIR=${OPENIDEA_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zshenv ]] && . $ZDOTDIR/.zshenv
OPENIDEA_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$_openidea_dir
"#;

const ZPROFILE: &str = r#"ZDOTDIR=$OPENIDEA_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zprofile ]] && . $ZDOTDIR/.zprofile
OPENIDEA_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$_openidea_dir
"#;

const ZSHRC: &str = r#"ZDOTDIR=$OPENIDEA_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zshrc ]] && . $ZDOTDIR/.zshrc
OPENIDEA_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$_openidea_dir
_openidea_osc7() { printf '\e]7;file://%s%s\a' "${HOST:-$(hostname)}" "$PWD"; }
precmd_functions=(_openidea_osc7 ${precmd_functions:#_openidea_osc7})
ZDOTDIR=${OPENIDEA_USER_ZDOTDIR:-$HOME}
unset OPENIDEA_USER_ZDOTDIR _openidea_dir
"#;

const ZLOGIN: &str = r#"ZDOTDIR=${OPENIDEA_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zlogin ]] && . $ZDOTDIR/.zlogin
unset OPENIDEA_USER_ZDOTDIR _openidea_dir
"#;

/// bash: a generated `--init-file` that replays the login sequence when the
/// launch carried `-l` (flag through the env var, since the rcfile replaces
/// the login startup file read), else `~/.bashrc`, then chains
/// `PROMPT_COMMAND` (dedup guard against a double hook after nested shells).
const BASHRC: &str = r#"if [ "${OPENIDEA_BASH_LOGIN:-}" = 1 ]; then
  unset OPENIDEA_BASH_LOGIN
  [ -f /etc/profile ] && . /etc/profile
  if [ -f ~/.bash_profile ]; then . ~/.bash_profile
  elif [ -f ~/.bash_login ]; then . ~/.bash_login
  elif [ -f ~/.profile ]; then . ~/.profile
  fi
else
  [ -f ~/.bashrc ] && . ~/.bashrc
fi
__openidea_osc7() { printf '\e]7;file://%s%s\a' "${HOSTNAME:-$(hostname)}" "$PWD"; }
case ";${PROMPT_COMMAND:-};" in
  *";__openidea_osc7;"*) ;;
  *) PROMPT_COMMAND="__openidea_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac
"#;

fn zsh_injection() -> Injection {
    let dir = scripts_dir().join("zsh");
    let written = (|| -> std::io::Result<()> {
        write_if_changed(&dir.join(".zshenv"), ZSHENV)?;
        write_if_changed(&dir.join(".zprofile"), ZPROFILE)?;
        write_if_changed(&dir.join(".zshrc"), ZSHRC)?;
        write_if_changed(&dir.join(".zlogin"), ZLOGIN)?;
        Ok(())
    })();
    if written.is_err() {
        return Injection { env: Vec::new(), args: Vec::new() };
    }
    let mut env = Vec::new();
    let user = std::env::var("ZDOTDIR").ok().filter(|v| !v.is_empty());
    if let Some(user) = user {
        env.push(("OPENIDEA_USER_ZDOTDIR".to_owned(), user));
    }
    env.push(("ZDOTDIR".to_owned(), dir.to_string_lossy().into_owned()));
    Injection { env, args: Vec::new() }
}

fn bash_injection(args: &[&str]) -> Injection {
    let rc = scripts_dir().join("bash").join("openidea.bashrc");
    if write_if_changed(&rc, BASHRC).is_err() {
        return Injection { env: Vec::new(), args: Vec::new() };
    }
    let mut env = Vec::new();
    if args.iter().any(|a| *a == "-l" || *a == "--login") {
        env.push(("OPENIDEA_BASH_LOGIN".to_owned(), "1".to_owned()));
    }
    Injection { env, args: vec!["--init-file".to_owned(), rc.to_string_lossy().into_owned(), "-i".to_owned()] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The family detection follows the basename, not the path.
    #[test]
    fn detect_follows_the_basename() {
        assert_eq!(detect("/bin/zsh"), ShellKind::Zsh);
        assert_eq!(detect("/usr/local/bin/bash"), ShellKind::Bash);
        assert_eq!(detect("/opt/homebrew/bin/fish"), ShellKind::Other);
        assert_eq!(detect("nu"), ShellKind::Other);
    }

    /// An explicit script/program argument means a non-interactive run — no
    /// hook may wrap it (it would break `zsh script.zsh` semantics).
    #[test]
    fn explicit_args_skip_the_injection() {
        assert!(inject(&ShellKind::Zsh, &["script.zsh"]).is_none());
        assert!(inject(&ShellKind::Bash, &["-c", "echo hi"]).is_none());
        assert!(inject(&ShellKind::Other, &["-l"]).is_none());
    }

    /// The zsh injection points `ZDOTDIR` at the generated set, the files
    /// exist, and `.zshrc` chains the user's rc plus the OSC 7 hook.
    #[test]
    fn zsh_injection_generates_a_chaining_rc() {
        let inj = inject(&ShellKind::Zsh, &["-l"]).expect("zsh injects");
        let zdotdir = inj.env.iter().find(|(k, _)| k == "ZDOTDIR").map(|(_, v)| v.clone()).expect("ZDOTDIR pair");
        let script = std::fs::read_to_string(std::path::Path::new(&zdotdir).join(".zshrc")).expect("generated .zshrc");
        assert!(script.contains(". $ZDOTDIR/.zshrc"), "chains the user's rc");
        assert!(script.contains("precmd_functions=(_openidea_osc7"));
        assert!(script.contains("\\e]7;file://"), "OSC 7 hook present");
        for name in [".zshenv", ".zprofile", ".zlogin"] {
            assert!(std::path::Path::new(&zdotdir).join(name).exists(), "{name} generated");
        }
    }

    /// The bash injection carries the init-file argv and replays login
    /// treatment when the launch was a login one.
    #[test]
    fn bash_injection_init_file_replays_login() {
        let inj = inject(&ShellKind::Bash, &["-l"]).expect("bash injects");
        assert_eq!(inj.args[0], "--init-file");
        let rc = std::path::Path::new(&inj.args[1]).to_path_buf();
        let script = std::fs::read_to_string(&rc).expect("generated rc");
        assert!(script.contains(". /etc/profile"), "login replay present");
        assert_eq!(inj.env.iter().find(|(k, _)| k == "OPENIDEA_BASH_LOGIN").map(|(_, v)| v.as_str()), Some("1"));
        assert!(script.contains("PROMPT_COMMAND=\"__openidea_osc7"));
    }

    /// A non-login bash replays only the user's rc.
    #[test]
    fn bash_injection_without_login_reads_bashrc() {
        let inj = inject(&ShellKind::Bash, &[]).expect("bash injects");
        let script = std::fs::read_to_string(&inj.args[1]).expect("generated rc");
        assert!(script.contains(". ~/.bashrc"));
        assert!(inj.env.is_empty(), "no login flag");
    }
}
