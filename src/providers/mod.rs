//! Providers: tarefas assíncronas que publicam o último snapshot num `watch`.
//! A UI só lê o valor mais recente; nenhuma fila cresce e nenhum provider bloqueia a tela.

pub mod calendar;
pub mod claude;
pub mod docker;
pub mod geo;
pub mod git;
pub mod ping;
pub mod specs;
pub mod system;
pub mod transcript;
pub mod usage;

use std::time::Duration;

use chrono::{DateTime, Utc};

/// Estado publicado por um provider: último dado bom + último erro.
/// Um erro não apaga o dado: o painel mostra o antigo marcado como desatualizado.
#[derive(Clone, Debug)]
pub struct Snapshot<T> {
    pub data: Option<T>,
    pub updated_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl<T> Default for Snapshot<T> {
    fn default() -> Self {
        Self { data: None, updated_at: None, error: None }
    }
}

impl<T> Snapshot<T> {
    pub fn ok(data: T) -> Self {
        Self { data: Some(data), updated_at: Some(Utc::now()), error: None }
    }

    pub fn is_loading(&self) -> bool {
        self.data.is_none() && self.error.is_none()
    }
}

/// Backoff exponencial para novas tentativas depois de falha.
pub struct Backoff {
    cur: Duration,
    min: Duration,
    max: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self { cur: min, min, max }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.cur;
        self.cur = (self.cur * 2).min(self.max);
        d
    }

    pub fn reset(&mut self) {
        self.cur = self.min;
    }
}

/// Evento de arquivo que indica mudança de verdade. O inotify também avisa de leituras
/// (abrir/fechar sem escrever) e de mudança de metadados; reagir a isso cria um loop,
/// porque o próprio provider lê os arquivos que observa.
pub fn is_change(ev: &notify::Event) -> bool {
    use notify::{
        EventKind,
        event::{AccessKind, AccessMode, ModifyKind},
    };
    match ev.kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        _ => false,
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(concat!("cockpit/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("cliente HTTP")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_changes_count() {
        use notify::{
            Event, EventKind,
            event::{AccessKind, AccessMode, CreateKind, MetadataKind, ModifyKind, RenameMode},
        };
        let ev = |k| Event::new(k);
        assert!(!is_change(&ev(EventKind::Access(AccessKind::Open(AccessMode::Any)))));
        assert!(!is_change(&ev(EventKind::Access(AccessKind::Close(AccessMode::Read)))));
        assert!(!is_change(&ev(EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)))));
        assert!(is_change(&ev(EventKind::Access(AccessKind::Close(AccessMode::Write)))));
        assert!(is_change(&ev(EventKind::Modify(ModifyKind::Name(RenameMode::To)))));
        assert!(is_change(&ev(EventKind::Create(CreateKind::File))));
    }

    #[test]
    fn backoff_doubles_until_max_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(30), Duration::from_secs(300));
        let got: Vec<u64> = (0..6).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(got, [30, 60, 120, 240, 300, 300]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 30);
    }
}
