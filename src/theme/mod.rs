//! Temas: tokens semânticos de cor, embutidos + instalados pelo usuário.

use std::{collections::BTreeMap, fs, path::Path, str::FromStr};

use anyhow::{Context, Result, anyhow};
use ratatui::style::Color;
use serde::Deserialize;

/// Tokens semânticos. Nenhum widget usa cor literal: tudo passa por aqui.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub highlight: Color,
    pub ok: Color,
    pub warn: Color,
    pub alert: Color,
    pub alert_bg: Color,
    pub alert_fg: Color,
    pub spark: Color,
    pub gauge_low: Color,
    pub gauge_mid: Color,
    pub gauge_high: Color,
    pub clock: Color,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

const TOKENS: &[&str] = &[
    "bg", "fg", "muted", "border", "accent", "highlight", "ok", "warn", "alert", "alert_bg",
    "alert_fg", "spark", "gauge_low", "gauge_mid", "gauge_high", "clock",
];

const BUILTIN: &[(&str, &str)] = &[
    ("dracula", include_str!("builtin/dracula.toml")),
    ("catppuccin-mocha", include_str!("builtin/catppuccin-mocha.toml")),
    ("tokyo-night", include_str!("builtin/tokyo-night.toml")),
    ("nord", include_str!("builtin/nord.toml")),
    ("gruvbox-dark", include_str!("builtin/gruvbox-dark.toml")),
    ("one-dark", include_str!("builtin/one-dark.toml")),
    ("solarized-dark", include_str!("builtin/solarized-dark.toml")),
    ("terminal", include_str!("builtin/terminal.toml")),
];

impl Theme {
    /// Lê um tema em TOML. Tokens ausentes herdam de outros (ex.: `alert_bg` ← `alert`),
    /// e o que sobrar herda do tema `terminal`, que usa só as cores ANSI.
    pub fn parse(src: &str, fallback_name: &str) -> Result<Self> {
        let file: ThemeFile = toml::from_str(src).context("TOML de tema inválido")?;
        if let Some(bad) = file.colors.keys().find(|k| !TOKENS.contains(&k.as_str())) {
            return Err(anyhow!("token desconhecido `{bad}` (válidos: {})", TOKENS.join(", ")));
        }
        let get = |k: &str| -> Result<Option<Color>> {
            file.colors
                .get(k)
                .map(|v| parse_color(v).with_context(|| format!("cor inválida em `{k}`: {v}")))
                .transpose()
        };
        let fg = get("fg")?.unwrap_or(Color::Reset);
        let alert = get("alert")?.unwrap_or(Color::LightRed);
        let ok = get("ok")?.unwrap_or(Color::Green);
        let warn = get("warn")?.unwrap_or(Color::Yellow);
        Ok(Self {
            name: file.name.unwrap_or_else(|| fallback_name.to_string()),
            bg: get("bg")?.unwrap_or(Color::Reset),
            fg,
            muted: get("muted")?.unwrap_or(Color::DarkGray),
            border: get("border")?.unwrap_or(Color::Gray),
            accent: get("accent")?.unwrap_or(Color::LightBlue),
            highlight: get("highlight")?.unwrap_or(Color::LightMagenta),
            ok,
            warn,
            alert,
            alert_bg: get("alert_bg")?.unwrap_or(alert),
            alert_fg: get("alert_fg")?.unwrap_or(Color::Black),
            spark: get("spark")?.unwrap_or(Color::Cyan),
            gauge_low: get("gauge_low")?.unwrap_or(ok),
            gauge_mid: get("gauge_mid")?.unwrap_or(warn),
            gauge_high: get("gauge_high")?.unwrap_or(alert),
            clock: get("clock")?.unwrap_or(fg),
        })
    }

    /// Converte cores RGB para a paleta de 256 quando o terminal não tem truecolor.
    pub fn downgraded(mut self) -> Self {
        for c in [
            &mut self.bg, &mut self.fg, &mut self.muted, &mut self.border, &mut self.accent,
            &mut self.highlight, &mut self.ok, &mut self.warn, &mut self.alert,
            &mut self.alert_bg, &mut self.alert_fg, &mut self.spark, &mut self.gauge_low,
            &mut self.gauge_mid, &mut self.gauge_high, &mut self.clock,
        ] {
            *c = to_256(*c);
        }
        self
    }
}

/// `#rrggbb`, nome ANSI (`red`, `lightblue`, `darkgray`…), índice `0..=255` ou `reset`.
pub fn parse_color(s: &str) -> Result<Color> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("reset") || s.eq_ignore_ascii_case("default") {
        return Ok(Color::Reset);
    }
    Color::from_str(s).map_err(|_| anyhow!("formato não reconhecido"))
}

fn to_256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    // Cubo 6×6×6 (16..=231) versus rampa de cinzas (232..=255): fica o mais próximo.
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let idx = |v: u8| STEPS.iter().enumerate().min_by_key(|(_, s)| (**s as i32 - v as i32).abs()).unwrap().0;
    let (ri, gi, bi) = (idx(r), idx(g), idx(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray_i = (((avg as i32 - 8).max(0)) / 10).min(23) as u8;
    let gray_v = 8 + 10 * gray_i;
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if dist((gray_v, gray_v, gray_v)) < dist(cube) {
        Color::Indexed(232 + gray_i)
    } else {
        Color::Indexed(16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8)
    }
}

/// Terminal aceita cor de 24 bits? `COLORTERM` é o padrão; o Windows Terminal não o define,
/// mas define `WT_SESSION`.
pub fn truecolor_supported() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    matches!(env("COLORTERM").as_str(), "truecolor" | "24bit")
        || !env("WT_SESSION").is_empty()
        || matches!(env("TERM_PROGRAM").as_str(), "vscode" | "iTerm.app" | "WezTerm" | "ghostty")
        || env("TERM").contains("kitty")
        || env("TERM").contains("alacritty")
}

pub fn is_builtin(name: &str) -> bool {
    BUILTIN.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// Link de arquivo no GitHub (`/blob/`) vira o endereço do conteúdo cru.
pub fn raw_url(url: &str) -> String {
    match url.strip_prefix("https://github.com/") {
        Some(rest) if rest.contains("/blob/") => format!("https://raw.githubusercontent.com/{}", rest.replacen("/blob/", "/", 1)),
        _ => url.to_string(),
    }
}

/// Nome de arquivo seguro: minúsculas, dígitos, `-` e `_`.
fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    s.trim_matches('-').to_string()
}

pub struct Installed {
    pub name: String,
    pub path: std::path::PathBuf,
    /// Substitui um tema embutido de mesmo nome.
    pub overrides_builtin: bool,
}

/// Valida o TOML de um tema e grava em `dir/<nome>.toml`, mantendo o texto original
/// (comentários inclusive). O nome vem do campo `name` ou, sem ele, de `fallback_name`.
pub fn install(src: &str, fallback_name: &str, dir: &Path, force: bool) -> Result<Installed> {
    let theme = Theme::parse(src, fallback_name).context("não é um tema válido")?;
    let name = slug(&theme.name);
    if name.is_empty() {
        return Err(anyhow!("o tema não tem nome; ponha `name = \"…\"` no arquivo"));
    }
    let path = dir.join(format!("{name}.toml"));
    if path.exists() && !force {
        return Err(anyhow!("{} já existe; use --force para substituir", path.display()));
    }
    fs::create_dir_all(dir)?;
    fs::write(&path, src).with_context(|| format!("gravar {}", path.display()))?;
    Ok(Installed { overrides_builtin: is_builtin(&name), name, path })
}

/// Remove um tema instalado (os embutidos não saem).
pub fn remove(name: &str, dir: &Path) -> Result<std::path::PathBuf> {
    let path = dir.join(format!("{}.toml", slug(name)));
    if !path.exists() {
        return Err(if is_builtin(name) {
            anyhow!("`{name}` é embutido e não pode ser removido")
        } else {
            anyhow!("não há tema instalado chamado `{name}` em {}", dir.display())
        });
    }
    fs::remove_file(&path)?;
    Ok(path)
}

#[derive(Debug)]
pub struct ThemeEntry {
    pub theme: Theme,
    pub builtin: bool,
}

/// Temas disponíveis, em ordem estável: embutidos primeiro, depois os do usuário.
/// Um tema do usuário com o mesmo nome de um embutido o substitui.
#[derive(Debug)]
pub struct ThemeRegistry {
    pub entries: Vec<ThemeEntry>,
    pub errors: Vec<String>,
}

impl ThemeRegistry {
    pub fn load(user_dir: &Path, truecolor: bool) -> Self {
        let mut entries: Vec<ThemeEntry> = BUILTIN
            .iter()
            .map(|(name, src)| ThemeEntry {
                theme: Theme::parse(src, name).expect("tema embutido inválido"),
                builtin: true,
            })
            .collect();
        let mut errors = Vec::new();

        if let Ok(dir) = fs::read_dir(user_dir) {
            let mut paths: Vec<_> = dir
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            paths.sort();
            for p in paths {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                match fs::read_to_string(&p).map_err(Into::into).and_then(|s| Theme::parse(&s, &stem)) {
                    Ok(theme) => match entries.iter_mut().find(|e| e.theme.name == theme.name) {
                        Some(e) => *e = ThemeEntry { theme, builtin: false },
                        None => entries.push(ThemeEntry { theme, builtin: false }),
                    },
                    Err(e) => errors.push(format!("{}: {e:#}", p.display())),
                }
            }
        }

        if !truecolor {
            for e in &mut entries {
                e.theme = e.theme.clone().downgraded();
            }
        }
        Self { entries, errors }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.theme.name.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, i: usize) -> &Theme {
        &self.entries[i % self.entries.len()].theme
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_cover_every_token() {
        for (name, src) in BUILTIN {
            let file: ThemeFile = toml::from_str(src).unwrap();
            for t in TOKENS {
                assert!(file.colors.contains_key(*t), "{name} sem `{t}`");
            }
            assert_eq!(Theme::parse(src, name).unwrap().name, *name);
        }
    }

    #[test]
    fn missing_tokens_inherit() {
        let t = Theme::parse("[colors]\nalert = \"#ff0000\"\nfg = \"#eeeeee\"", "x").unwrap();
        assert_eq!(t.name, "x");
        assert_eq!(t.alert_bg, Color::Rgb(255, 0, 0));
        assert_eq!(t.gauge_high, Color::Rgb(255, 0, 0));
        assert_eq!(t.clock, Color::Rgb(0xee, 0xee, 0xee));
        assert_eq!(t.bg, Color::Reset);
    }

    #[test]
    fn rejects_unknown_token_and_bad_color() {
        assert!(Theme::parse("[colors]\nbackground = \"#000000\"", "x").is_err());
        assert!(Theme::parse("[colors]\nbg = \"#zzzzzz\"", "x").is_err());
    }

    #[test]
    fn downgrade_maps_to_256() {
        assert_eq!(to_256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(to_256(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(to_256(Color::Rgb(0x80, 0x80, 0x80)), Color::Indexed(244));
        assert_eq!(to_256(Color::Red), Color::Red);
    }

    #[test]
    fn installs_validates_and_removes() {
        let dir = std::env::temp_dir().join(format!("cockpit-install-{}", std::process::id()));
        let src = "# meu tema\nname = \"Meu Tema!\"\n[colors]\nbg = \"#101010\"\n";
        let i = install(src, "x", &dir, false).unwrap();
        assert_eq!(i.name, "meu-tema");
        assert!(!i.overrides_builtin);
        assert_eq!(fs::read_to_string(&i.path).unwrap(), src, "mantém o texto original");
        assert!(install(src, "x", &dir, false).is_err(), "sem --force não sobrescreve");
        assert!(install(src, "x", &dir, true).is_ok());
        assert!(install("[colors]\nbg = \"verde-limão\"", "ruim", &dir, false).is_err());
        let d = install("[colors]\nbg = \"#000000\"", "dracula", &dir, false).unwrap();
        assert!(d.overrides_builtin, "sem `name`, usa o nome do arquivo");
        remove("meu-tema", &dir).unwrap();
        assert!(remove("meu-tema", &dir).is_err());
        assert!(remove("nord", &dir).unwrap_err().to_string().contains("embutido"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn github_links_become_raw() {
        assert_eq!(
            raw_url("https://github.com/dracula/x/blob/main/themes/a.toml"),
            "https://raw.githubusercontent.com/dracula/x/main/themes/a.toml"
        );
        assert_eq!(raw_url("https://example.com/a.toml"), "https://example.com/a.toml");
    }

    #[test]
    fn user_theme_overrides_builtin() {
        let dir = std::env::temp_dir().join(format!("cockpit-themes-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("dracula.toml"), "name = \"dracula\"\n[colors]\nbg = \"#000000\"").unwrap();
        fs::write(dir.join("meu.toml"), "[colors]\nbg = \"#111111\"").unwrap();
        fs::write(dir.join("ruim.toml"), "[colors]\nbg = 3").unwrap();
        let reg = ThemeRegistry::load(&dir, true);
        fs::remove_dir_all(&dir).ok();
        let d = reg.index_of("dracula").unwrap();
        assert_eq!(reg.get(d).bg, Color::Rgb(0, 0, 0));
        assert!(!reg.entries[d].builtin);
        assert!(reg.index_of("meu").is_some());
        assert_eq!(reg.errors.len(), 1);
        assert_eq!(reg.len(), BUILTIN.len() + 1);
    }
}
