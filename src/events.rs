use crate::util::{chomp, JsonStrMap};
use chrono::{DateTime, Local};
use std::borrow::Cow;
use std::net::SocketAddr;

pub(crate) enum Event {
    ConnectStart {
        timestamp: DateTime<Local>,
        host: String,
        port: u16,
    },
    ConnectFinish {
        timestamp: DateTime<Local>,
        peer: SocketAddr,
    },
    TlsStart {
        timestamp: DateTime<Local>,
    },
    TlsFinish {
        timestamp: DateTime<Local>,
    },
    Recv {
        timestamp: DateTime<Local>,
        data: String,
    },
    Send {
        timestamp: DateTime<Local>,
        data: String,
    },
    Disconnect {
        timestamp: DateTime<Local>,
    },
}

impl Event {
    pub(crate) fn connect_start(host: &str, port: u16) -> Self {
        Event::ConnectStart {
            timestamp: Local::now(),
            host: String::from(host),
            port,
        }
    }

    pub(crate) fn connect_finish(peer: SocketAddr) -> Self {
        Event::ConnectFinish {
            timestamp: Local::now(),
            peer,
        }
    }

    pub(crate) fn tls_start() -> Self {
        Event::TlsStart {
            timestamp: Local::now(),
        }
    }

    pub(crate) fn tls_finish() -> Self {
        Event::TlsFinish {
            timestamp: Local::now(),
        }
    }

    pub(crate) fn recv(data: String) -> Self {
        Event::Recv {
            timestamp: Local::now(),
            data,
        }
    }

    pub(crate) fn send(data: String) -> Self {
        Event::Send {
            timestamp: Local::now(),
            data,
        }
    }

    pub(crate) fn disconnect() -> Self {
        Event::Disconnect {
            timestamp: Local::now(),
        }
    }

    pub(crate) fn timestamp(&self) -> &DateTime<Local> {
        match self {
            Event::ConnectStart { timestamp, .. } => timestamp,
            Event::ConnectFinish { timestamp, .. } => timestamp,
            Event::TlsStart { timestamp } => timestamp,
            Event::TlsFinish { timestamp } => timestamp,
            Event::Recv { timestamp, .. } => timestamp,
            Event::Send { timestamp, .. } => timestamp,
            Event::Disconnect { timestamp } => timestamp,
        }
    }

    //pub(crate) fn display_time(&self) -> String {
    //    self.timestamp().format("%H:%M:%S").to_string()
    //}

    pub(crate) fn sigil(&self) -> char {
        match self {
            Event::Recv { .. } => '<',
            Event::Send { .. } => '>',
            _ => '*',
        }
    }

    pub(crate) fn message(&self) -> Cow<'_, str> {
        match self {
            Event::ConnectStart { .. } => Cow::from("Connecting ..."),
            Event::ConnectFinish { peer, .. } => Cow::from(format!("Connected to {peer}")),
            Event::TlsStart { .. } => Cow::from("Initializing TLS ..."),
            Event::TlsFinish { .. } => Cow::from("TLS established"),
            Event::Recv { data, .. } => Cow::from(chomp(data)),
            Event::Send { data, .. } => Cow::from(chomp(data)),
            Event::Disconnect { .. } => Cow::from("Disconnected"),
        }
    }

    pub(crate) fn to_json(&self) -> String {
        let json = JsonStrMap::new().field("timestamp", &self.timestamp().to_rfc3339());
        match self {
            Event::ConnectStart { host, port, .. } => json
                .field("event", "connection-start")
                .field("host", host)
                .raw_field("port", &port.to_string())
                .finish(),
            Event::ConnectFinish { peer, .. } => json
                .field("event", "connection-complete")
                .field("host", &peer.ip())
                .raw_field("port", &peer.port().to_string())
                .finish(),
            Event::TlsStart { .. } => json.field("event", "tls-start").finish(),
            Event::TlsFinish { .. } => json.field("event", "tls-complete").finish(),
            Event::Recv { data, .. } => json.field("event", "recv").field("data", data).finish(),
            Event::Send { data, .. } => json.field("event", "send").field("data", data).finish(),
            Event::Disconnect { .. } => json.field("event", "disconnect").finish(),
        }
    }
}
