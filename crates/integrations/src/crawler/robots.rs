//! What a host's robots.txt allows the Mascate to read (RFC 9309): the
//! group that names the Mascate, else the one for every robot; the longest
//! matching rule wins, `allow` on a tie; `*` matches anything and `$` ends
//! a rule. `Crawl-delay`, which the RFC leaves out but sites still write,
//! is honoured too.

use std::time::Duration;

/// The rules of one robots.txt for one robot. The default allows
/// everything, as a site without a robots.txt does.
#[derive(Debug, Default)]
pub(super) struct Robots {
    rules: Vec<Rule>,
    delay: Option<Duration>,
}

#[derive(Debug)]
struct Rule {
    allow: bool,
    pattern: String,
}

#[derive(Default)]
struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
    delay: Option<Duration>,
}

impl Robots {
    /// The rules in `text` for the robot whose product token is `token`, in
    /// lower case.
    pub(super) fn parse(text: &str, token: &str) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        let mut naming_agents = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "user-agent" => {
                    if !naming_agents {
                        groups.push(Group::default());
                        naming_agents = true;
                    }
                    if let Some(group) = groups.last_mut() {
                        group.agents.push(value.to_ascii_lowercase());
                    }
                }
                key @ ("allow" | "disallow") => {
                    naming_agents = false;
                    // An empty disallow allows everything, like no rule.
                    if let (Some(group), false) = (groups.last_mut(), value.is_empty()) {
                        group.rules.push(Rule {
                            allow: key == "allow",
                            pattern: value.to_owned(),
                        });
                    }
                }
                "crawl-delay" => {
                    naming_agents = false;
                    if let Some(group) = groups.last_mut() {
                        group.delay = value
                            .parse::<f64>()
                            .ok()
                            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                            .map(Duration::from_secs_f64);
                    }
                }
                _ => {}
            }
        }
        let names = |group: &Group, wanted: &str| {
            group
                .agents
                .iter()
                .any(|agent| agent.split('/').next().unwrap_or_default().trim() == wanted)
        };
        let wanted = if groups.iter().any(|group| names(group, token)) {
            token
        } else {
            "*"
        };
        let mut robots = Robots::default();
        for group in groups.into_iter().filter(|group| names(group, wanted)) {
            robots.rules.extend(group.rules);
            robots.delay = robots.delay.max(group.delay);
        }
        robots
    }

    /// Whether `path`, with its query, may be read.
    pub(super) fn allows(&self, path: &str) -> bool {
        if path == "/robots.txt" {
            return true;
        }
        self.rules
            .iter()
            .filter(|rule| matches(&rule.pattern, path))
            .max_by_key(|rule| (rule.pattern.len(), rule.allow))
            .is_none_or(|rule| rule.allow)
    }

    /// How long to wait between requests, when the site says.
    pub(super) fn delay(&self) -> Option<Duration> {
        self.delay
    }
}

/// Whether the rule `pattern` matches `path`: from its start, `*` standing
/// for anything and a final `$` for the end of the path.
fn matches(pattern: &str, path: &str) -> bool {
    let (pattern, to_the_end) = match pattern.strip_suffix('$') {
        Some(pattern) => (pattern, true),
        None => (pattern, false),
    };
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return !to_the_end || rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    if to_the_end {
        rest.ends_with(last)
    } else {
        rest.contains(last)
    }
}
