//! Integração com o shell. Um programa não consegue mudar a pasta do shell que o abriu,
//! então o `cockpit` grava o destino num arquivo e uma função do shell faz o `cd`
//! depois que ele sai (o mesmo truque do yazi e do broot).
//!
//! Formato do arquivo: linha 1 = pasta; linha 2 (opcional) = `continue` ou `resume:<id>`.
//! A função só aceita esses dois comandos: nada do arquivo é executado com `eval`.

use std::{fs, io::Write, path::Path};

use anyhow::{Result, bail};

/// O que fazer depois de sair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitTarget {
    pub dir: std::path::PathBuf,
    pub then: Option<Then>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Then {
    /// `claude --continue`
    Continue,
    /// `claude --resume <id>`
    Resume(String),
}

impl ExitTarget {
    pub fn to_file_contents(&self) -> String {
        let mut s = format!("{}\n", self.dir.display());
        match &self.then {
            Some(Then::Continue) => s.push_str("continue\n"),
            Some(Then::Resume(id)) => s.push_str(&format!("resume:{id}\n")),
            None => {}
        }
        s
    }

    /// A linha equivalente para colar no shell (quando não há wrapper).
    pub fn command_line(&self) -> String {
        let dir = shell_quote(&self.dir.display().to_string());
        match &self.then {
            None => format!("cd {dir}"),
            Some(Then::Continue) => format!("cd {dir} && claude --continue"),
            Some(Then::Resume(id)) => format!("cd {dir} && claude --resume {id}"),
        }
    }
}

pub fn write_cd_file(path: &Path, target: &ExitTarget) -> Result<()> {
    fs::write(path, target.to_file_contents())?;
    Ok(())
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

const POSIX: &str = r#"# cockpit: Enter no seletor troca a pasta deste shell.
cockpit() {
  local tmp dir cmd
  tmp="$(mktemp -t cockpit.XXXXXX)" || return
  command cockpit --cd-file "$tmp" "$@"
  local rc=$?
  { IFS= read -r dir; IFS= read -r cmd; } < "$tmp"
  rm -f -- "$tmp"
  if [ -n "$dir" ] && [ -d "$dir" ]; then
    cd -- "$dir" || return
    case "$cmd" in
      continue) claude --continue ;;
      resume:*) id="${cmd#resume:}"
                case "$id" in *[!0-9a-fA-F-]*|'') ;; *) claude --resume "$id" ;; esac ;;
    esac
  fi
  return $rc
}
"#;

const FISH: &str = r#"# cockpit: Enter no seletor troca a pasta deste shell.
function cockpit
    set -l tmp (mktemp -t cockpit.XXXXXX); or return
    command cockpit --cd-file $tmp $argv
    set -l rc $status
    set -l lines (cat $tmp)
    rm -f -- $tmp
    if test -n "$lines[1]"; and test -d "$lines[1]"
        cd -- $lines[1]; or return
        if test (count $lines) -ge 2
            switch $lines[2]
                case continue
                    claude --continue
                case 'resume:*'
                    set -l id (string replace 'resume:' '' -- $lines[2])
                    if string match -qr '^[0-9a-fA-F-]+$' -- $id
                        claude --resume $id
                    end
            end
        end
    end
    return $rc
end
"#;

pub fn init_script(shell: &str) -> Result<&'static str> {
    Ok(match shell {
        "zsh" | "bash" | "sh" => POSIX,
        "fish" => FISH,
        other => bail!("shell `{other}` não suportado (use zsh, bash ou fish)"),
    })
}

/// Copia texto para a área de transferência pelo terminal (OSC 52). Funciona no Windows
/// Terminal, iTerm2, kitty, WezTerm e dentro do tmux com `set-clipboard on`.
pub fn osc52_copy(text: &str) {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

pub(crate) fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cd_file_format() {
        let t = ExitTarget { dir: PathBuf::from("/home/u/v360"), then: None };
        assert_eq!(t.to_file_contents(), "/home/u/v360\n");
        let t = ExitTarget { dir: PathBuf::from("/x"), then: Some(Then::Resume("ab-12".into())) };
        assert_eq!(t.to_file_contents(), "/x\nresume:ab-12\n");
        assert_eq!(t.command_line(), "cd /x && claude --resume ab-12");
        let t = ExitTarget { dir: PathBuf::from("/tmp/it's here"), then: Some(Then::Continue) };
        assert_eq!(t.command_line(), r"cd '/tmp/it'\''s here' && claude --continue");
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn scripts_exist() {
        assert!(init_script("zsh").unwrap().contains("command cockpit --cd-file"));
        assert!(init_script("fish").unwrap().contains("function cockpit"));
        assert!(init_script("tcsh").is_err());
    }
}
