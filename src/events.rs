use crate::util::{chomp, display_vis, JsonStrMap};
use crossterm::style::{StyledContent, Stylize};
use std::fmt;
use std::net::SocketAddr;
use time::format_description::well_known::Rfc3339;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

pub(crate) static HMS_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]:[second]");

pub(crate) enum Event {
    ConnectStart {
        timestamp: OffsetDateTime,
        host: String,
        port: u16,
    },
    ConnectFinish {
        timestamp: OffsetDateTime,
        peer: SocketAddr,
    },
    TlsStart {
        timestamp: OffsetDateTime,
    },
    TlsFinish {
        timestamp: OffsetDateTime,
    },
    Recv {
        timestamp: OffsetDateTime,
        data: String,
    },
    Send {
        timestamp: OffsetDateTime,
        data: String,
    },
    Disconnect {
        timestamp: OffsetDateTime,
    },
    Error {
        timestamp: OffsetDateTime,
        data: anyhow::Error,
    },
}

impl Event {
    pub(crate) fn connect_start(host: &str, port: u16) -> Self {
        Event::ConnectStart {
            timestamp: now(),
            host: String::from(host),
            port,
        }
    }

    pub(crate) fn connect_finish(peer: SocketAddr) -> Self {
        Event::ConnectFinish {
            timestamp: now(),
            peer,
        }
    }

    pub(crate) fn tls_start() -> Self {
        Event::TlsStart { timestamp: now() }
    }

    pub(crate) fn tls_finish() -> Self {
        Event::TlsFinish { timestamp: now() }
    }

    pub(crate) fn recv(data: String) -> Self {
        Event::Recv {
            timestamp: now(),
            data,
        }
    }

    pub(crate) fn send(data: String) -> Self {
        Event::Send {
            timestamp: now(),
            data,
        }
    }

    pub(crate) fn disconnect() -> Self {
        Event::Disconnect { timestamp: now() }
    }

    pub(crate) fn error(data: anyhow::Error) -> Self {
        Event::Error {
            timestamp: now(),
            data,
        }
    }

    pub(crate) fn timestamp(&self) -> &OffsetDateTime {
        match self {
            Event::ConnectStart { timestamp, .. } => timestamp,
            Event::ConnectFinish { timestamp, .. } => timestamp,
            Event::TlsStart { timestamp } => timestamp,
            Event::TlsFinish { timestamp } => timestamp,
            Event::Recv { timestamp, .. } => timestamp,
            Event::Send { timestamp, .. } => timestamp,
            Event::Disconnect { timestamp } => timestamp,
            Event::Error { timestamp, .. } => timestamp,
        }
    }

    pub(crate) fn display_time(&self) -> String {
        self.timestamp().format(&HMS_FMT).unwrap()
    }

    pub(crate) fn sigil(&self) -> char {
        match self {
            Event::Recv { .. } => '<',
            Event::Send { .. } => '>',
            Event::Error { .. } => '!',
            _ => '*',
        }
    }

    pub(crate) fn to_message(&self, time: bool) -> EventDisplay<'_> {
        EventDisplay { event: self, time }
    }

    fn message_chunks(&self) -> Vec<StyledContent<String>> {
        match self {
            Event::ConnectStart { .. } => vec![String::from("Connecting ...").stylize()],
            Event::ConnectFinish { peer, .. } => vec![format!("Connected to {peer}").stylize()],
            Event::TlsStart { .. } => vec![String::from("Initializing TLS ...").stylize()],
            Event::TlsFinish { .. } => vec![String::from("TLS established").stylize()],
            Event::Recv { data, .. } => display_vis(chomp(data)),
            Event::Send { data, .. } => display_vis(chomp(data)),
            Event::Disconnect { .. } => vec![String::from("Disconnected").stylize()],
            Event::Error { data, .. } => vec![format!("{data:#}").stylize()],
        }
    }

    pub(crate) fn to_json(&self) -> String {
        let json =
            JsonStrMap::new().field("timestamp", &self.timestamp().format(&Rfc3339).unwrap());
        match self {
            Event::ConnectStart { host, port, .. } => json
                .field("event", "connection-start")
                .field("host", host)
                .raw_field("port", &port.to_string())
                .finish(),
            Event::ConnectFinish { peer, .. } => json
                .field("event", "connection-complete")
                .field("peer_ip", &peer.ip())
                .finish(),
            Event::TlsStart { .. } => json.field("event", "tls-start").finish(),
            Event::TlsFinish { .. } => json.field("event", "tls-complete").finish(),
            Event::Recv { data, .. } => json.field("event", "recv").field("data", data).finish(),
            Event::Send { data, .. } => json.field("event", "send").field("data", data).finish(),
            Event::Disconnect { .. } => json.field("event", "disconnect").finish(),
            Event::Error { data, .. } => json
                .field("event", "error")
                .field("data", &format!("{data:#}"))
                .finish(),
        }
    }
}

pub(crate) struct EventDisplay<'a> {
    event: &'a Event,
    time: bool,
}

impl<'a> fmt::Display for EventDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.time {
            write!(f, "[{}] ", self.event.display_time())?;
        }
        write!(f, "{} ", self.event.sigil())?;
        for chunk in self.event.message_chunks() {
            write!(f, "{chunk}")?;
        }
        Ok(())
    }
}

pub(crate) fn now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}
