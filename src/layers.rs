//! The eleven layers of `setup.yaml`: which lists are diffed by id, which lists are plain
//! string sets, and which scalar objects are compared as a whole.

use crate::util::get_path;
use crate::Doc;
use indexmap::IndexMap;

#[derive(Debug, Clone, Copy)]
pub struct Collection {
    /// Dotted path into the document, e.g. `tools.mcp_servers`.
    pub path: &'static str,
    /// `Some("id")` for lists of objects keyed by id; `None` for lists of strings.
    pub key: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct Layer {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub collections: &'static [Collection],
    pub scalars: &'static [&'static str],
}

const fn keyed(path: &'static str) -> Collection {
    Collection {
        path,
        key: Some("id"),
    }
}
const fn plain(path: &'static str) -> Collection {
    Collection { path, key: None }
}

pub static LAYERS: [Layer; 11] = [
    Layer {
        id: "identity",
        title: "Identity",
        summary: "Who the assistant acts for, and every account/credential it can use, by role. Names of secrets only.",
        collections: &[keyed("identity.accounts")],
        scalars: &[],
    },
    Layer {
        id: "brain",
        title: "Brain",
        summary: "Models, runtimes, the roster of other AI workers, quality gates and budgets.",
        collections: &[keyed("brain.runtimes"), keyed("brain.roster"), keyed("brain.gates")],
        scalars: &["brain.claude_code"],
    },
    Layer {
        id: "instructions",
        title: "Instructions",
        summary: "Rules the assistant follows: id, scope, hard|standing, date, reason. Plus the files they come from.",
        collections: &[keyed("instructions.sources"), keyed("instructions.rules")],
        scalars: &[],
    },
    Layer {
        id: "tools",
        title: "Tools",
        summary: "MCP servers, skills, plugins, hooks, slash commands and the permission allow/deny/ask lists.",
        collections: &[
            keyed("tools.mcp_servers"),
            keyed("tools.skills"),
            keyed("tools.plugins"),
            keyed("tools.hooks"),
            keyed("tools.commands"),
            plain("tools.permissions.allow"),
            plain("tools.permissions.deny"),
            plain("tools.permissions.ask"),
            plain("tools.denied_mcp_servers"),
        ],
        scalars: &[],
    },
    Layer {
        id: "loops",
        title: "Loops",
        summary: "Every scheduled/background job (launchd, cron, ...) including disabled and removed ones, with schedule, owner and success metric.",
        collections: &[keyed("loops")],
        scalars: &[],
    },
    Layer {
        id: "channels",
        title: "Channels",
        summary: "Where the assistant talks and writes: chat, email, social, calendar, todo, journal.",
        collections: &[
            keyed("channels.slack"),
            keyed("channels.email"),
            keyed("channels.social"),
            keyed("channels.calendar"),
            keyed("channels.todo"),
            keyed("channels.journal"),
        ],
        scalars: &[],
    },
    Layer {
        id: "projects",
        title: "Projects",
        summary: "Ongoing work the assistant knows about: products, repos, side projects, with status.",
        collections: &[keyed("projects")],
        scalars: &[],
    },
    Layer {
        id: "business",
        title: "Business lanes",
        summary: "Revenue or outreach lanes: goal, metric, status, and which loops/channels serve them.",
        collections: &[keyed("business.lanes")],
        scalars: &[],
    },
    Layer {
        id: "data",
        title: "Data & state",
        summary: "Memory stores, ledgers, logs and other state files, by path. Contents are never copied in.",
        collections: &[keyed("data.state"), keyed("data.references")],
        scalars: &["data.memory"],
    },
    Layer {
        id: "infra",
        title: "Infra",
        summary: "Machine, installed CLIs, local repos and hosted services the setup depends on.",
        collections: &[keyed("infra.clis"), keyed("infra.repos"), keyed("infra.services")],
        scalars: &["infra.machine"],
    },
    Layer {
        id: "safety",
        title: "Safety policy",
        summary: "Machine-checkable policies (deny_mcp, no_payment_tools, forbidden_paths, required_gate, ...).",
        collections: &[keyed("safety.policies")],
        scalars: &[],
    },
];

/// Item counts for one layer: the total and each collection (named by its path minus the
/// layer prefix, or the full path for top-level lists).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LayerCount {
    pub total: usize,
    pub per: IndexMap<String, usize>,
}

/// Count the items in every collection of every layer.
pub fn count_layers(doc: &Doc) -> IndexMap<String, LayerCount> {
    let mut out = IndexMap::new();
    for layer in LAYERS.iter() {
        let mut per = IndexMap::new();
        let mut total = 0;
        for c in layer.collections {
            let n = get_path(doc, c.path)
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let name = c
                .path
                .split_once('.')
                .map(|(_, rest)| rest)
                .unwrap_or(c.path);
            per.insert(name.to_string(), n);
            total += n;
        }
        out.insert(layer.id.to_string(), LayerCount { total, per });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_every_layer() {
        let d = json!({ "tools": { "mcp_servers": [{ "id": "a" }], "permissions": { "allow": ["x", "y"] } }, "loops": [{ "id": "l" }] });
        let c = count_layers(&d);
        assert_eq!(c["tools"].total, 3);
        assert_eq!(c["tools"].per["permissions.allow"], 2);
        assert_eq!(c["loops"].per["loops"], 1);
        assert_eq!(c["safety"].total, 0);
        assert_eq!(c.len(), 11);
    }
}
