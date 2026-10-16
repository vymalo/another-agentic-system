//! Roles, permissions and the one question the application asks of them: may this person do this
//! to that (ADR 0033).
//!
//! The model is pure: no I/O, no clock, no ports. A [`Policy`] maps role names, as an identity
//! provider spells them (a group, a realm role), to a [`RoleGrant`]: the [`Permission`]s the role
//! holds and the agents they are about. A person's [`Access`] is the union of the grants of the roles they carry, and
//! [`Policy::allows`] / [`Policy::check`] answer for one [`Resource`].
//!
//! **Fail closed.** A role the policy does not name grants nothing. A person none of whose roles
//! is named gets the policy's *default role* (`auth.defaultRole`), and nothing at all when there is
//! none. A permission over a resource of another kind (`thread.read` asked of an agent) is
//! denied.
//!
//! **Own, and nothing else.** A permission over threads reaches the threads the person owns (the
//! owner is the e-mail, [`Principal::user`]) and no others: **no role, `admin` included, reads or
//! acts on another person's thread** (ADR 0039, which reverses owner decision 4 of ADR 0033). There
//! is no wider [`Scope`] to grant, so a configuration cannot grant one by accident. `admin` is an
//! operational permission that gives no access to what people wrote.
//!
//! **An agent a registry lists has a second condition, its audience** (ADR 0045, decision 1). To
//! read it (`agent.read`) or invoke it (`agent.invoke`) a person needs **both** that a role of
//! theirs grants the permission over the agent's id (`auth.roles`, as always), **and** that the
//! agent's [`AgentAudience`] lets them in: it holds `"*"`, or one of the values of their roles
//! claim, or a role of theirs holds `admin`. An agent with no audience is for administrators only
//! (fail closed). The deployment's own agents have no audience and keep the first condition alone.
//! Both conditions are asked in one place, [`Access::allows`] over a [`Resource::Agent`] that
//! carries the agent's audience, so a check cannot be made without it.
//!
//! **Reading through a link is a second check, not a scope** (ADR 0040). A [`Resource::SharedThread`]
//! is a thread somebody else shared: `thread.read` (and `artifact.read` for its files) is granted
//! over it, at any role, when the thread is *effectively* shared with signed-in people or wider,
//! and nothing else is: no `thread.write`, no `thread.share`, no `admin`. A public thread is read
//! by anybody and asks no one's roles, so it never comes here.
//!
//! Two answers, because they are told apart on the wire: a person whose roles do not hold a
//! permission at all is [`Denied::Permission`] (403: the answer does not depend on what is asked
//! for, so it leaks nothing), and one who holds it but not over this resource is
//! [`Denied::OutOfScope`] (a thread that is not the person's is then 404, so its existence never
//! leaks).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use orch_core::{AccessTier, AgentId, UserId, Visibility};
use orch_ports::{AgentAudience, Principal, Role};

/// What a role may be granted. The names follow the platform's vocabulary
/// (`another-agentic-platform` `docs/architecture/08-security.md`, section 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Permission {
    /// `agent.read`: see an agent in the list and read its card.
    AgentRead,
    /// `agent.invoke`: start a thread on an agent, or send it a message.
    AgentInvoke,
    /// `thread.read`: read threads: the thread, its log and stream, its export, its branches.
    ThreadRead,
    /// `thread.write`: act on threads: start one, send, answer, cancel, rename, describe, fork.
    ThreadWrite,
    /// `thread.share`: set, widen, narrow or rotate the visibility of one's own thread (ADR 0040).
    /// It takes no scope (the only thread a person can act on is their own). Taking a link down is
    /// not gated by it: the owner can always revoke.
    ThreadShare,
    /// `thread.delete`: delete one's own thread, which erases it with its edits and its files (ADR
    /// 0043). It takes no scope (the only thread a person can erase is their own), and **it does not
    /// need `thread.write`**: a person who may only read may still erase their own data. A
    /// deployment can withhold it (a legal hold, for instance); the people in such a role are erased
    /// by the operator.
    ThreadDelete,
    /// `artifact.read`: download the files of the threads the person may read.
    ArtifactRead,
    /// `admin`: operational and content-free (ADR 0039). It reaches no thread, no file and no
    /// listing of other people's; it names what a future endpoint may show an operator, which is
    /// never a person's content.
    Admin,
    /// `tools.user.add`: add, edit, authorize and re-key an MCP server of one's own (ADR 0046). It
    /// takes no scope (a person's servers are the person's own), and **it is granted to no role by
    /// default: `admin` does not imply it**, the built-in `user` and `admin` do not hold it, and a
    /// deployment lists it for the roles it chooses. Listing, attaching and removing one's own
    /// servers are not gated by it, so a person whose permission is taken away can still remove what
    /// they added.
    ToolsUserAdd,
}

impl Permission {
    /// Every permission, in the order they are listed.
    pub const ALL: [Permission; 9] = [
        Permission::AgentRead,
        Permission::AgentInvoke,
        Permission::ThreadRead,
        Permission::ThreadWrite,
        Permission::ThreadShare,
        Permission::ThreadDelete,
        Permission::ArtifactRead,
        Permission::Admin,
        Permission::ToolsUserAdd,
    ];

    /// The name used in the configuration and by `GET /api/me`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::AgentRead => "agent.read",
            Permission::AgentInvoke => "agent.invoke",
            Permission::ThreadRead => "thread.read",
            Permission::ThreadWrite => "thread.write",
            Permission::ThreadShare => "thread.share",
            Permission::ThreadDelete => "thread.delete",
            Permission::ArtifactRead => "artifact.read",
            Permission::Admin => "admin",
            Permission::ToolsUserAdd => "tools.user.add",
        }
    }

    /// The permission named `name`, exactly.
    pub fn parse(name: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.as_str() == name)
    }

    /// Whether the permission is over threads and so has a scope.
    pub const fn is_scoped(self) -> bool {
        matches!(
            self,
            Permission::ThreadRead | Permission::ThreadWrite | Permission::ArtifactRead
        )
    }

    /// Whether the permission is over agents and so is limited by a role's `agents`.
    pub const fn is_over_agents(self) -> bool {
        matches!(self, Permission::AgentRead | Permission::AgentInvoke)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How far a permission over threads reaches: the threads the person owns, and no others (ADR
/// 0039). The type has one value on purpose: a wider reach is not a thing this application can be
/// told to grant, and `GET /api/me` still says `own` for a client that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// The threads the person owns.
    Own,
}

impl Scope {
    /// The name used in the configuration and by `GET /api/me`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::Own => "own",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which agents a role's agent permissions are about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentScope {
    /// Every agent (`"*"`).
    All,
    /// These agents, by id, and no other.
    Only(BTreeSet<AgentId>),
}

impl AgentScope {
    /// The scope the agent ids and `"*"` of a configuration say: `"*"` anywhere in the list is
    /// every agent, an empty list is no agent.
    pub fn from_patterns<I, S>(patterns: I) -> AgentScope
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut only = BTreeSet::new();
        for pattern in patterns {
            let pattern = pattern.as_ref().trim();
            if pattern == "*" {
                return AgentScope::All;
            }
            if !pattern.is_empty() {
                only.insert(AgentId::new(pattern));
            }
        }
        AgentScope::Only(only)
    }

    /// Whether the scope covers `agent`.
    pub fn admits(&self, agent: &AgentId) -> bool {
        match self {
            AgentScope::All => true,
            AgentScope::Only(ids) => ids.contains(agent),
        }
    }

    /// The scope with every agent id it names replaced by `canonical(id)`: a role that names an
    /// agent by an alias (ADR 0049) is about the agent the alias names.
    #[must_use]
    pub fn canonicalized(self, canonical: impl Fn(&AgentId) -> AgentId) -> AgentScope {
        match self {
            AgentScope::All => AgentScope::All,
            AgentScope::Only(ids) => AgentScope::Only(ids.iter().map(canonical).collect()),
        }
    }

    /// The scope that covers what either covers.
    #[must_use]
    pub fn union(self, other: AgentScope) -> AgentScope {
        match (self, other) {
            (AgentScope::All, _) | (_, AgentScope::All) => AgentScope::All,
            (AgentScope::Only(mut a), AgentScope::Only(b)) => {
                a.extend(b);
                AgentScope::Only(a)
            }
        }
    }

    /// The patterns of the scope as a configuration writes them: `["*"]`, or the ids in order.
    pub fn patterns(&self) -> Vec<String> {
        match self {
            AgentScope::All => vec!["*".to_owned()],
            AgentScope::Only(ids) => ids.iter().map(ToString::to_string).collect(),
        }
    }
}

/// What one role grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrant {
    /// The permissions the role holds.
    pub permissions: BTreeSet<Permission>,
    /// The highest access tier the role may pick for a message (`auth.roles.<role>.maxTier`, ADR
    /// 0061): a ceiling, not a permission. `None`: no cap.
    pub max_tier: Option<AccessTier>,
    /// The agents `agent.read` and `agent.invoke` are about.
    pub agents: AgentScope,
}

impl RoleGrant {
    /// A role that grants nothing.
    pub fn none() -> RoleGrant {
        RoleGrant {
            permissions: BTreeSet::new(),
            max_tier: None,
            agents: AgentScope::Only(BTreeSet::new()),
        }
    }

    /// The role of an ordinary person: everything except `admin`, over their own threads and
    /// every agent.
    pub fn user() -> RoleGrant {
        RoleGrant {
            permissions: BTreeSet::from([
                Permission::AgentRead,
                Permission::AgentInvoke,
                Permission::ThreadRead,
                Permission::ThreadWrite,
                Permission::ThreadShare,
                Permission::ThreadDelete,
                Permission::ArtifactRead,
            ]),
            max_tier: None,
            agents: AgentScope::All,
        }
    }

    /// The administrator: a user who also holds `admin`. Operational and content-free (ADR 0039):
    /// the administrator reads and acts on their own threads, as a user does, and on nobody's else.
    pub fn admin() -> RoleGrant {
        let mut grant = RoleGrant::user();
        grant.permissions.insert(Permission::Admin);
        grant
    }

    fn holds(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// Why a [`Policy`] cannot be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyError {
    /// The default role names a role the policy does not define: a person would get nothing, and
    /// the operator would not know why.
    #[error("the default role {0:?} is not one of the roles")]
    UnknownDefaultRole(String),
}

/// Which roles grant what (`auth.roles`), and the role of a person none of whose roles is known
/// (`auth.defaultRole`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    roles: BTreeMap<Role, RoleGrant>,
    default_role: Option<Role>,
}

/// The roles of a deployment that defines none (`auth.roles` absent): `user` and `admin`, as ADR
/// 0033 gives them.
pub fn built_in_roles() -> BTreeMap<Role, RoleGrant> {
    BTreeMap::from([
        (Role::new("user"), RoleGrant::user()),
        (Role::new("admin"), RoleGrant::admin()),
    ])
}

impl Default for Policy {
    /// The policy of a deployment that configures none: `user` and `admin` as the ADR gives them,
    /// and everyone who has no known role is a `user`. In the proxy-header mode nobody has a role,
    /// so this is what keeps that mode as it was: everyone is a user.
    fn default() -> Self {
        Policy {
            roles: built_in_roles(),
            default_role: Some(Role::new("user")),
        }
    }
}

impl Policy {
    /// A policy of `roles`, with `default_role` for a person none of whose roles is among them.
    ///
    /// # Errors
    /// [`PolicyError::UnknownDefaultRole`] when `default_role` is not one of `roles`.
    pub fn new(
        roles: BTreeMap<Role, RoleGrant>,
        default_role: Option<Role>,
    ) -> Result<Policy, PolicyError> {
        if let Some(role) = &default_role
            && !roles.contains_key(role)
        {
            return Err(PolicyError::UnknownDefaultRole(role.to_string()));
        }
        Ok(Policy {
            roles,
            default_role,
        })
    }

    /// The policy with the agent ids of every role replaced by `canonical(id)`, so that a role
    /// written with an alias of an agent (ADR 0049) grants what it says for the agent. A request
    /// asks about the canonical id ([`App::canonical_agent`](crate::App::canonical_agent)).
    #[must_use]
    pub fn with_canonical_agents(mut self, canonical: impl Fn(&AgentId) -> AgentId) -> Policy {
        for grant in self.roles.values_mut() {
            let agents = std::mem::replace(&mut grant.agents, AgentScope::All);
            grant.agents = agents.canonicalized(&canonical);
        }
        self
    }

    /// A policy that grants nothing to anybody.
    pub fn deny_all() -> Policy {
        Policy {
            roles: BTreeMap::new(),
            default_role: None,
        }
    }

    /// The role names the policy defines.
    pub fn role_names(&self) -> impl Iterator<Item = &Role> {
        self.roles.keys()
    }

    /// The default role, if there is one.
    pub fn default_role(&self) -> Option<&Role> {
        self.default_role.as_ref()
    }

    /// Whether somebody could invoke `agent`, whose audience is `audience`: some role of the
    /// policy lets its holders invoke it (`agent.invoke` over an agent scope that names it), and
    /// the audience can let somebody in. For a caller that acts for a person whose roles it does
    /// not have (a role is a claim of a request's credential, never stored): the question that can
    /// be asked is whether anyone could, so an agent no role names is refused for everybody.
    ///
    /// The values of an audience are names the identity provider gives and the policy does not
    /// know, so an audience of `"*"` or of values can let somebody in; one that is for
    /// administrators only ([`AgentAudience::AdminsOnly`]) can only when some role of the policy
    /// holds `admin`. Whom it lets in is [`Access::allows`]'s to say, with the person's roles.
    pub fn any_role_may_invoke(&self, agent: &AgentId, audience: &AgentAudience) -> bool {
        let covered = self
            .roles
            .values()
            .any(|grant| grant.holds(Permission::AgentInvoke) && grant.agents.admits(agent));
        covered
            && match audience {
                AgentAudience::Static | AgentAudience::Everyone | AgentAudience::Roles(_) => true,
                AgentAudience::AdminsOnly => self
                    .roles
                    .values()
                    .any(|grant| grant.holds(Permission::Admin)),
            }
    }

    /// What `who` may do: the union of the grants of the roles they carry that the policy knows,
    /// or the default role's when none is known.
    pub fn access<'p>(&'p self, who: &impl Requester) -> Access<'p> {
        let mut roles: Vec<(&'p Role, &'p RoleGrant)> = who
            .roles()
            .into_iter()
            .filter_map(|role| self.roles.get_key_value(role))
            .collect();
        if roles.is_empty()
            && let Some(role) = &self.default_role
            && let Some(entry) = self.roles.get_key_value(role)
        {
            roles.push(entry);
        }
        roles.sort_by(|a, b| a.0.cmp(b.0));
        roles.dedup_by(|a, b| a.0 == b.0);
        Access {
            user: who.user().clone(),
            claims: who.roles().into_iter().cloned().collect(),
            roles,
        }
    }

    /// Whether `who` may use `permission` on `resource`. A convenience for
    /// [`Access::allows`] when one question is asked.
    pub fn allows(
        &self,
        who: &impl Requester,
        permission: Permission,
        resource: &Resource<'_>,
    ) -> bool {
        self.access(who).allows(permission, resource)
    }

    /// [`Access::check`] for one question.
    ///
    /// # Errors
    /// [`Denied`], telling a permission the person lacks from a resource out of its reach.
    pub fn check(
        &self,
        who: &impl Requester,
        permission: Permission,
        resource: &Resource<'_>,
    ) -> Result<(), Denied> {
        self.access(who).check(permission, resource)
    }
}

/// What a request is from: a user, and the roles the credential carried. A [`Principal`] is one;
/// a bare [`UserId`] is a person whose credential carried no roles (a thread's owner acting in a
/// test, or through the proxy header), and so has the policy's default role.
pub trait Requester {
    /// The person: the key of what they own.
    fn user(&self) -> &UserId;

    /// The roles the credential carried, as an identity provider spelled them.
    fn roles(&self) -> Vec<&Role>;
}

impl Requester for Principal {
    fn user(&self) -> &UserId {
        &self.user
    }

    fn roles(&self) -> Vec<&Role> {
        self.roles.iter().collect()
    }
}

impl Requester for UserId {
    fn user(&self) -> &UserId {
        self
    }

    fn roles(&self) -> Vec<&Role> {
        Vec::new()
    }
}

/// What a permission is used on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource<'a> {
    /// A thread of `owner`, or what belongs to it (its log, its files).
    Thread {
        /// The thread's owner.
        owner: &'a UserId,
    },
    /// A thread its owner shared, read by somebody else through its link (ADR 0040). `effective`
    /// is what is served now: the thread's visibility capped by the deployment's.
    SharedThread {
        /// `min(visibility, cap)`.
        effective: Visibility,
    },
    /// An agent, with whom it is for (ADR 0045): `agent.read` and `agent.invoke` are asked of an
    /// agent together with its audience, which the registry's entry holds. Asking only the roles'
    /// side of the question, before the registry has been asked, is [`Access::check_scope`].
    Agent {
        /// The agent's id.
        id: &'a AgentId,
        /// Whom the agent is for: [`AgentAudience::Static`] for one of the deployment's own, the
        /// entry's audience for one a registry lists.
        audience: &'a AgentAudience,
    },
    /// Nothing in particular: the permission itself (`admin`, or the right to start a thread).
    Anything,
}

/// Why a permission was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// The person's roles do not hold the permission at all. What is asked for does not matter,
    /// so the answer says nothing about it.
    Permission(Permission),
    /// The roles hold the permission, not over this resource: another person's thread, or an
    /// agent a role's `agents` do not name.
    OutOfScope(Permission),
    /// The roles hold the permission over the agent, and the agent's audience leaves the person
    /// out (ADR 0045). An agent that is for people the person is not one of must look, to a person
    /// who asks for it by name, like an agent that does not exist: a caller answers this as it
    /// answers an agent the registry does not list, unless the person already has the agent in a
    /// thread of their own.
    Audience(Permission),
}

/// What one person may do: the grants of their known roles. Built by [`Policy::access`].
#[derive(Debug, Clone)]
pub struct Access<'p> {
    user: UserId,
    /// Every role the credential carried, known to the policy or not, as the identity provider
    /// spelled them: what an agent's audience is read against (ADR 0045). Unlike `roles`, which
    /// holds only the roles the policy maps to permissions.
    claims: Vec<Role>,
    roles: Vec<(&'p Role, &'p RoleGrant)>,
}

impl<'p> Access<'p> {
    /// The roles that count: the known roles the credential carried, or the default role.
    pub fn roles(&self) -> impl Iterator<Item = &'p Role> + '_ {
        self.roles.iter().map(|(role, _)| *role)
    }

    /// Whether nothing at all is granted: no role of the person is known and there is no default
    /// role (or the roles that count hold no permission).
    pub fn is_empty(&self) -> bool {
        self.roles.iter().all(|(_, g)| g.permissions.is_empty())
    }

    /// Whether any role holds `permission`, over anything.
    pub fn has(&self, permission: Permission) -> bool {
        self.roles.iter().any(|(_, g)| g.holds(permission))
    }

    /// The highest access tier the person may pick for a message to **any** agent (ADR 0061): over
    /// the roles that hold `agent.invoke`, the highest cap, and none when any of them has no cap.
    /// A role that cannot start an agent does not lift it. A hint for the screen (`GET /api/me`);
    /// what is checked when a message is written is [`Access::max_tier_for`].
    pub fn max_tier(&self) -> Option<AccessTier> {
        Self::highest(
            self.roles
                .iter()
                .filter(|(_, grant)| grant.holds(Permission::AgentInvoke))
                .map(|(_, grant)| grant.max_tier),
        )
    }

    /// The highest access tier the person may pick for a message to `agent` (ADR 0061): over the
    /// roles that hold `agent.invoke` **on that agent**, the highest cap, and none (no cap) when
    /// any of them has none. A role that does not let the person invoke the agent does not lift it.
    pub fn max_tier_for(&self, agent: &AgentId) -> Option<AccessTier> {
        Self::highest(
            self.roles
                .iter()
                .filter(|(_, grant)| {
                    grant.holds(Permission::AgentInvoke) && grant.agents.admits(agent)
                })
                .map(|(_, grant)| grant.max_tier),
        )
    }

    /// The highest of some caps, `None` (no cap) when one of them is none, or when there are none.
    fn highest(caps: impl Iterator<Item = Option<AccessTier>>) -> Option<AccessTier> {
        let mut highest: Option<AccessTier> = None;
        for cap in caps {
            let cap = cap?;
            highest = Some(highest.map_or(cap, |h| h.max(cap)));
        }
        highest
    }

    /// The scope of `permission` when a role holds it: [`Scope::Own`] for one over threads;
    /// `None` when no role holds it or it has no scope (see [`Permission::is_scoped`]).
    pub fn scope(&self, permission: Permission) -> Option<Scope> {
        (self.has(permission) && permission.is_scoped()).then_some(Scope::Own)
    }

    /// The agents `permission` (`agent.read` or `agent.invoke`) is about, over every role that
    /// holds it: no agent when none does.
    pub fn agents(&self, permission: Permission) -> AgentScope {
        self.roles
            .iter()
            .filter(|(_, g)| g.holds(permission))
            .fold(AgentScope::Only(BTreeSet::new()), |all, (_, g)| {
                all.union(g.agents.clone())
            })
    }

    /// Every permission held, with its scope when it has one, in the order of [`Permission::ALL`].
    pub fn permissions(&self) -> Vec<(Permission, Option<Scope>)> {
        Permission::ALL
            .into_iter()
            .filter(|p| self.has(*p))
            .map(|p| (p, self.scope(p)))
            .collect()
    }

    /// Whether the person may use `permission` on `resource`. Any one role may say yes; a
    /// permission asked of a resource of another kind is no.
    ///
    /// **For an agent this is the one rule of ADR 0045, decision 1.4**: some role grants the
    /// permission over the agent's id, **and** the agent's audience lets the person in
    /// ([`meets_audience`](Self::meets_audience)). Every check of an agent goes through here.
    pub fn allows(&self, permission: Permission, resource: &Resource<'_>) -> bool {
        self.granted(permission, resource) && self.audience_allows(resource)
    }

    /// Whether some role grants `permission` over `resource` (the roles' half of [`allows`](Self::allows)).
    fn granted(&self, permission: Permission, resource: &Resource<'_>) -> bool {
        self.roles
            .iter()
            .any(|(_, grant)| self.grant_allows(grant, permission, resource))
    }

    /// Whether the audience of an agent resource lets the person in; true for every other
    /// resource, which has none.
    fn audience_allows(&self, resource: &Resource<'_>) -> bool {
        match resource {
            Resource::Agent { audience, .. } => self.meets_audience(audience),
            Resource::Thread { .. } | Resource::SharedThread { .. } | Resource::Anything => true,
        }
    }

    /// Whether `audience` lets the person in (ADR 0045, decision 1.4): it is the deployment's own
    /// agent's ([`AgentAudience::Static`]), or `"*"`, or it holds one of the values of the roles
    /// claim the credential carried (byte for byte, whether the policy knows that role or not), or
    /// a role of the person holds `admin`. An agent with no audience is therefore for
    /// administrators only.
    ///
    /// This is half of [`allows`](Self::allows) and never enough on its own: it says nothing of
    /// `auth.roles`.
    pub fn meets_audience(&self, audience: &AgentAudience) -> bool {
        match audience {
            AgentAudience::Static | AgentAudience::Everyone => true,
            AgentAudience::AdminsOnly => self.has(Permission::Admin),
            AgentAudience::Roles(wanted) => {
                self.has(Permission::Admin)
                    || self
                        .claims
                        .iter()
                        .any(|role| wanted.contains(role.as_str()))
            }
        }
    }

    /// [`check`](Self::check) with the roles' half only, for an agent known by its id alone: the
    /// question that can be asked before the registry is, so that what a role's `agents` refuse is
    /// refused for every id alike, listed or not. **It never lets a person use an agent a registry
    /// lists**: when the registry's entry is at hand, ask [`check`](Self::check) with it.
    ///
    /// # Errors
    /// As [`check`](Self::check), without [`Denied::Audience`].
    pub fn check_scope(&self, permission: Permission, id: &AgentId) -> Result<(), Denied> {
        self.check(
            permission,
            &Resource::Agent {
                id,
                audience: &AgentAudience::Static,
            },
        )
    }

    fn grant_allows(
        &self,
        grant: &RoleGrant,
        permission: Permission,
        resource: &Resource<'_>,
    ) -> bool {
        if !grant.holds(permission) {
            return false;
        }
        match (permission, resource) {
            (
                Permission::ThreadRead | Permission::ThreadWrite | Permission::ArtifactRead,
                Resource::Thread { owner },
            ) => **owner == self.user,
            // Sharing is the owner's act, over their own thread.
            (Permission::ThreadShare, Resource::Thread { owner }) => **owner == self.user,
            // Erasing is the owner's act, over their own thread.
            (Permission::ThreadDelete, Resource::Thread { owner }) => **owner == self.user,
            // The link is the grant: any role that reads, at any scope, reads a thread that is
            // effectively shared with signed-in people or wider. Nothing else is granted over it.
            (
                Permission::ThreadRead | Permission::ArtifactRead,
                Resource::SharedThread { effective },
            ) => *effective >= Visibility::Internal,
            // Starting a thread is not about anyone's: it is a thread of the person's own.
            (Permission::ThreadWrite, Resource::Anything) => true,
            (Permission::AgentRead | Permission::AgentInvoke, Resource::Agent { id, .. }) => {
                grant.agents.admits(id)
            }
            (Permission::AgentRead | Permission::AgentInvoke, Resource::Anything) => {
                // "Some agent", as far as the role goes: the listing, then filtered by agent.
                match &grant.agents {
                    AgentScope::All => true,
                    AgentScope::Only(ids) => !ids.is_empty(),
                }
            }
            (Permission::Admin, Resource::Anything) => true,
            // Adding a server is not about anyone's: it is a server of the person's own.
            (Permission::ToolsUserAdd, Resource::Anything) => true,
            // A permission asked of a resource it is not about.
            _ => false,
        }
    }

    /// [`allows`](Self::allows), saying why not.
    ///
    /// # Errors
    /// [`Denied::Permission`] when no role holds `permission`, [`Denied::OutOfScope`] when
    /// some does but none over `resource`, and [`Denied::Audience`] when one does and the audience
    /// of an agent leaves the person out.
    pub fn check(&self, permission: Permission, resource: &Resource<'_>) -> Result<(), Denied> {
        if !self.has(permission) {
            Err(Denied::Permission(permission))
        } else if !self.granted(permission, resource) {
            Err(Denied::OutOfScope(permission))
        } else if !self.audience_allows(resource) {
            Err(Denied::Audience(permission))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn user(email: &str) -> UserId {
        UserId::new(email)
    }

    fn principal(email: &str, roles: &[&str]) -> Principal {
        Principal {
            user: user(email),
            email: Some(email.to_owned()),
            name: None,
            roles: roles.iter().map(|r| Role::new(*r)).collect(),
            expires_at: None,
        }
    }

    fn thread_of(owner: &UserId) -> Resource<'_> {
        Resource::Thread { owner }
    }

    #[test]
    fn permission_names_round_trip_and_nothing_else_parses() {
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(p));
            assert_eq!(p.to_string(), p.as_str());
        }
        for bad in ["", "thread", "Thread.Read", "thread.read ", "thread.*", "*"] {
            assert_eq!(Permission::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn scoped_permissions_are_the_ones_over_threads() {
        let scoped: Vec<_> = Permission::ALL
            .into_iter()
            .filter(|p| p.is_scoped())
            .collect();
        assert_eq!(
            scoped,
            [
                Permission::ThreadRead,
                Permission::ThreadWrite,
                Permission::ArtifactRead
            ]
        );
        let agents: Vec<_> = Permission::ALL
            .into_iter()
            .filter(|p| p.is_over_agents())
            .collect();
        assert_eq!(agents, [Permission::AgentRead, Permission::AgentInvoke]);
    }

    #[test]
    fn agent_patterns() {
        assert_eq!(AgentScope::from_patterns(["*"]), AgentScope::All);
        assert_eq!(AgentScope::from_patterns(["coder", "*"]), AgentScope::All);
        let only = AgentScope::from_patterns([" coder ", "researcher", ""]);
        assert!(only.admits(&AgentId::new("coder")));
        assert!(only.admits(&AgentId::new("researcher")));
        assert!(!only.admits(&AgentId::new("chat")));
        // No glob: a pattern that is not `*` names one agent.
        let prefix = AgentScope::from_patterns(["cod*"]);
        assert!(!prefix.admits(&AgentId::new("coder")));
        let none = AgentScope::from_patterns(Vec::<String>::new());
        assert!(!none.admits(&AgentId::new("coder")));
        assert_eq!(only.patterns(), ["coder", "researcher"]);
        assert_eq!(AgentScope::All.patterns(), ["*"]);
        assert_eq!(
            only.clone()
                .union(AgentScope::from_patterns(["chat"]))
                .patterns(),
            ["chat", "coder", "researcher"]
        );
        assert_eq!(only.union(AgentScope::All), AgentScope::All);
    }

    // The matrix: who, what they ask, over whose thread. Alice and Bob are users, Root is an
    // administrator. Every row is one decision of the default policy: nobody reaches another
    // person's thread (ADR 0039), and the administrator is no exception.
    #[test]
    fn the_default_policy_over_threads() {
        let policy = Policy::default();
        let (alice, bob) = (user("alice@x.io"), user("bob@x.io"));
        let a = principal("alice@x.io", &["user"]);
        let root = principal("root@x.io", &["admin"]);
        // (who, permission, owner of the thread, allowed)
        let rows: [(&Principal, Permission, &UserId, bool); 12] = [
            (&a, Permission::ThreadRead, &alice, true),
            (&a, Permission::ThreadRead, &bob, false),
            (&a, Permission::ThreadWrite, &alice, true),
            (&a, Permission::ThreadWrite, &bob, false),
            (&a, Permission::ArtifactRead, &alice, true),
            (&a, Permission::ArtifactRead, &bob, false),
            // The administrator reads and acts on their own, as a user does, and on no one's else.
            (&root, Permission::ThreadRead, &alice, false),
            (&root, Permission::ThreadRead, &user("root@x.io"), true),
            (&root, Permission::ThreadWrite, &alice, false),
            (&root, Permission::ThreadWrite, &user("root@x.io"), true),
            (&root, Permission::ArtifactRead, &bob, false),
            (&root, Permission::ArtifactRead, &user("root@x.io"), true),
        ];
        for (who, permission, owner, expected) in rows {
            assert_eq!(
                policy.allows(who, permission, &thread_of(owner)),
                expected,
                "{} {permission} on a thread of {owner}",
                who.user
            );
        }
    }

    #[test]
    fn only_the_administrator_holds_admin() {
        let policy = Policy::default();
        assert!(!policy.allows(
            &principal("a@x.io", &["user"]),
            Permission::Admin,
            &Resource::Anything
        ));
        assert!(policy.allows(
            &principal("r@x.io", &["admin"]),
            Permission::Admin,
            &Resource::Anything
        ));
        // Roles are compared exactly.
        for spelled in ["Admin", "ADMIN", "admin ", "administrator", "realm:admin"] {
            let p = principal("r@x.io", &[spelled]);
            // Unknown roles grant nothing, so the person is a plain user (the default role).
            assert!(
                !policy.allows(&p, Permission::Admin, &Resource::Anything),
                "{spelled}"
            );
        }
    }

    #[test]
    fn a_permission_asked_of_a_resource_it_is_not_about_is_denied() {
        let policy = Policy::default();
        let root = principal("root@x.io", &["admin"]);
        let owner = user("root@x.io");
        let coder = AgentId::new("coder");
        for permission in Permission::ALL {
            for resource in [
                Resource::Thread { owner: &owner },
                Resource::SharedThread {
                    effective: Visibility::Internal,
                },
                Resource::Agent {
                    id: &coder,
                    audience: &AgentAudience::Static,
                },
                Resource::Anything,
            ] {
                let expected = matches!(
                    (permission, &resource),
                    (
                        Permission::ThreadRead
                            | Permission::ThreadWrite
                            | Permission::ThreadShare
                            | Permission::ThreadDelete
                            | Permission::ArtifactRead,
                        Resource::Thread { .. }
                    ) | (
                        Permission::ThreadRead | Permission::ArtifactRead,
                        Resource::SharedThread { .. }
                    ) | (
                        Permission::AgentRead | Permission::AgentInvoke,
                        Resource::Agent { .. } | Resource::Anything
                    ) | (
                        Permission::Admin | Permission::ThreadWrite,
                        Resource::Anything
                    )
                );
                assert_eq!(
                    policy.allows(&root, permission, &resource),
                    expected,
                    "{permission} on {resource:?}"
                );
            }
        }
    }

    /// ADR 0040: reading through a link is a second check. Who (a user, an administrator, a role
    /// that reads and nothing else, a role that grants nothing) over a thread somebody else
    /// shared, at each effective visibility, for each permission that could be asked.
    #[test]
    fn the_shared_thread_matrix() {
        let reader_only = Policy::new(
            BTreeMap::from([
                (Role::new("user"), RoleGrant::user()),
                (Role::new("admin"), RoleGrant::admin()),
                (
                    Role::new("reader"),
                    RoleGrant {
                        permissions: BTreeSet::from([Permission::ThreadRead]),
                        ..RoleGrant::none()
                    },
                ),
                (Role::new("nobody"), RoleGrant::none()),
            ]),
            None,
        )
        .unwrap();
        let alice = principal("alice@x.io", &["user"]);
        let root = principal("root@x.io", &["admin"]);
        let reader = principal("rita@x.io", &["reader"]);
        let nobody = principal("nick@x.io", &["nobody"]);
        let stranger = principal("sam@x.io", &["wizard"]);
        use Visibility::{Internal, Private, Public};
        // (who, effective, thread.read, artifact.read)
        let rows: [(&Principal, Visibility, bool, bool); 15] = [
            (&alice, Private, false, false),
            (&alice, Internal, true, true),
            (&alice, Public, true, true),
            // an administrator has no more reach than anybody else
            (&root, Private, false, false),
            (&root, Internal, true, true),
            (&root, Public, true, true),
            // a role that reads, and cannot read files
            (&reader, Private, false, false),
            (&reader, Internal, true, false),
            (&reader, Public, true, false),
            // a role that grants nothing, or that nothing names (and there is no default role)
            (&nobody, Internal, false, false),
            (&nobody, Public, false, false),
            (&stranger, Internal, false, false),
            (&stranger, Public, false, false),
            (&stranger, Private, false, false),
            (&nobody, Private, false, false),
        ];
        for (who, effective, read, files) in rows {
            let resource = Resource::SharedThread { effective };
            assert_eq!(
                reader_only.allows(who, Permission::ThreadRead, &resource),
                read,
                "{} thread.read on a {effective} thread",
                who.user
            );
            assert_eq!(
                reader_only.allows(who, Permission::ArtifactRead, &resource),
                files,
                "{} artifact.read on a {effective} thread",
                who.user
            );
            // neither the link nor any role grants more than reading: no writing, no sharing, no
            // admin over a thread that is somebody else's
            for permission in [
                Permission::ThreadWrite,
                Permission::ThreadShare,
                Permission::ThreadDelete,
                Permission::Admin,
                Permission::AgentInvoke,
            ] {
                assert!(
                    !reader_only.allows(who, permission, &resource),
                    "{} {permission} on a {effective} thread",
                    who.user
                );
            }
        }
    }

    /// ADR 0040: `thread.share` is the owner's, over their own thread, and takes no scope.
    #[test]
    fn sharing_is_the_owners_and_has_no_scope() {
        let policy = Policy::default();
        let (alice, bob) = (user("alice@x.io"), user("bob@x.io"));
        let a = principal("alice@x.io", &["user"]);
        let root = principal("root@x.io", &["admin"]);
        assert!(policy.allows(&a, Permission::ThreadShare, &thread_of(&alice)));
        assert_eq!(
            policy.check(&a, Permission::ThreadShare, &thread_of(&bob)),
            Err(Denied::OutOfScope(Permission::ThreadShare))
        );
        // the administrator shares their own, and nobody else's
        assert!(policy.allows(
            &root,
            Permission::ThreadShare,
            &thread_of(&user("root@x.io"))
        ));
        assert!(!policy.allows(&root, Permission::ThreadShare, &thread_of(&alice)));
        assert_eq!(policy.access(&a).scope(Permission::ThreadShare), None);
        assert!(!Permission::ThreadShare.is_scoped());
        assert_eq!(
            Permission::parse("thread.share"),
            Some(Permission::ThreadShare)
        );
        // a role that lacks the permission shares nothing, its own threads included
        let no_share = Policy::new(
            BTreeMap::from([(
                Role::new("user"),
                RoleGrant {
                    permissions: RoleGrant::user()
                        .permissions
                        .into_iter()
                        .filter(|p| *p != Permission::ThreadShare)
                        .collect(),
                    ..RoleGrant::user()
                },
            )]),
            Some(Role::new("user")),
        )
        .unwrap();
        assert_eq!(
            no_share.check(&a, Permission::ThreadShare, &thread_of(&alice)),
            Err(Denied::Permission(Permission::ThreadShare))
        );
    }

    /// ADR 0043: `thread.delete` is the owner's, over their own thread, takes no scope, and is
    /// held by the built-in `user` and `admin` roles without `thread.write`.
    #[test]
    fn deleting_is_the_owners_has_no_scope_and_needs_no_write_permission() {
        let policy = Policy::default();
        let (alice, bob) = (user("alice@x.io"), user("bob@x.io"));
        let a = principal("alice@x.io", &["user"]);
        let root = principal("root@x.io", &["admin"]);
        assert!(policy.allows(&a, Permission::ThreadDelete, &thread_of(&alice)));
        assert_eq!(
            policy.check(&a, Permission::ThreadDelete, &thread_of(&bob)),
            Err(Denied::OutOfScope(Permission::ThreadDelete))
        );
        // the administrator erases their own, and nobody else's
        assert!(policy.allows(
            &root,
            Permission::ThreadDelete,
            &thread_of(&user("root@x.io"))
        ));
        assert!(!policy.allows(&root, Permission::ThreadDelete, &thread_of(&alice)));
        assert_eq!(policy.access(&a).scope(Permission::ThreadDelete), None);
        assert!(!Permission::ThreadDelete.is_scoped());
        assert_eq!(
            Permission::parse("thread.delete"),
            Some(Permission::ThreadDelete)
        );
        // a role that only reads may still erase its own data
        let reader = Policy::new(
            BTreeMap::from([(
                Role::new("reader"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::ThreadRead, Permission::ThreadDelete]),
                    ..RoleGrant::none()
                },
            )]),
            Some(Role::new("reader")),
        )
        .unwrap();
        let r = principal("alice@x.io", &["reader"]);
        assert!(reader.allows(&r, Permission::ThreadDelete, &thread_of(&alice)));
        assert!(!reader.allows(&r, Permission::ThreadWrite, &thread_of(&alice)));
        // a role that lists its permissions without it erases nothing, its own threads included
        let no_delete = Policy::new(
            BTreeMap::from([(
                Role::new("user"),
                RoleGrant {
                    permissions: RoleGrant::user()
                        .permissions
                        .into_iter()
                        .filter(|p| *p != Permission::ThreadDelete)
                        .collect(),
                    ..RoleGrant::user()
                },
            )]),
            Some(Role::new("user")),
        )
        .unwrap();
        assert_eq!(
            no_delete.check(&a, Permission::ThreadDelete, &thread_of(&alice)),
            Err(Denied::Permission(Permission::ThreadDelete))
        );
    }

    /// ADR 0046: `tools.user.add` is granted to no role by default. The built-in `user` and `admin`
    /// do not hold it, and `admin` does not imply it; a role that lists it holds it, over nothing in
    /// particular (a server of the person's own), with no scope; a role that lists its permissions
    /// without it adds nothing.
    #[test]
    fn adding_a_server_of_ones_own_is_granted_to_no_role_by_default() {
        let policy = Policy::default();
        let a = principal("alice@x.io", &["user"]);
        let root = principal("root@x.io", &["admin"]);
        for who in [&a, &root] {
            assert!(
                !policy.access(who).has(Permission::ToolsUserAdd),
                "a built-in role holds tools.user.add"
            );
            assert_eq!(
                policy.check(who, Permission::ToolsUserAdd, &Resource::Anything),
                Err(Denied::Permission(Permission::ToolsUserAdd))
            );
        }
        for grant in [RoleGrant::user(), RoleGrant::admin()] {
            assert!(!grant.permissions.contains(&Permission::ToolsUserAdd));
        }
        // admin is a user with `admin`, and `admin` alone gives nothing else
        let admin_only = Policy::new(
            BTreeMap::from([(
                Role::new("ops"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::Admin]),
                    ..RoleGrant::none()
                },
            )]),
            Some(Role::new("ops")),
        )
        .unwrap();
        let ops = principal("ops@x.io", &["ops"]);
        assert!(admin_only.allows(&ops, Permission::Admin, &Resource::Anything));
        assert!(!admin_only.allows(&ops, Permission::ToolsUserAdd, &Resource::Anything));

        // a role that lists it
        let maker = Policy::new(
            BTreeMap::from([
                (Role::new("user"), RoleGrant::user()),
                (
                    Role::new("maker"),
                    RoleGrant {
                        permissions: BTreeSet::from([Permission::ToolsUserAdd]),
                        ..RoleGrant::none()
                    },
                ),
            ]),
            Some(Role::new("user")),
        )
        .unwrap();
        let m = principal("alice@x.io", &["maker"]);
        assert!(maker.allows(&m, Permission::ToolsUserAdd, &Resource::Anything));
        // it is not about a thread or an agent: asked of those it is no
        let (alice, coder) = (user("alice@x.io"), AgentId::new("coder"));
        assert!(!maker.allows(&m, Permission::ToolsUserAdd, &thread_of(&alice)));
        assert!(!maker.allows(
            &m,
            Permission::ToolsUserAdd,
            &Resource::Agent {
                id: &coder,
                audience: &AgentAudience::Static
            }
        ));
        // and it gives nothing else
        assert!(!maker.allows(&m, Permission::ThreadRead, &thread_of(&alice)));
        // a person with both roles has it, and everything the user has
        let both = principal("alice@x.io", &["user", "maker"]);
        assert!(maker.allows(&both, Permission::ToolsUserAdd, &Resource::Anything));
        assert!(maker.allows(&both, Permission::ThreadRead, &thread_of(&alice)));

        assert_eq!(maker.access(&m).scope(Permission::ToolsUserAdd), None);
        assert!(!Permission::ToolsUserAdd.is_scoped());
        assert!(!Permission::ToolsUserAdd.is_over_agents());
        assert_eq!(
            Permission::parse("tools.user.add"),
            Some(Permission::ToolsUserAdd)
        );
        assert_eq!(Permission::ToolsUserAdd.to_string(), "tools.user.add");
        assert!(Permission::ALL.contains(&Permission::ToolsUserAdd));
    }

    #[test]
    fn an_unknown_role_grants_nothing_and_falls_back_to_the_default_role() {
        let policy = Policy::default();
        let alice = user("alice@x.io");
        let stranger = principal("alice@x.io", &["wizard"]);
        // Nothing the unknown role says counts; the person is what the default role makes them.
        let access = policy.access(&stranger);
        assert_eq!(
            access.roles().map(Role::as_str).collect::<Vec<_>>(),
            ["user"]
        );
        assert!(access.allows(Permission::ThreadRead, &thread_of(&alice)));
        assert!(!access.has(Permission::Admin));

        // Without a default role the same person has nothing at all.
        let strict = Policy::new(
            BTreeMap::from([(Role::new("user"), RoleGrant::user())]),
            None,
        )
        .unwrap();
        let access = strict.access(&stranger);
        assert!(access.is_empty());
        assert_eq!(access.roles().count(), 0);
        for permission in Permission::ALL {
            assert!(!access.has(permission));
            assert_eq!(access.scope(permission), None);
            assert_eq!(
                access.check(permission, &Resource::Anything),
                Err(Denied::Permission(permission))
            );
        }
        // A known role is not replaced by the default.
        assert!(
            !strict
                .access(&principal("a@x.io", &["user", "wizard"]))
                .is_empty()
        );
    }

    #[test]
    fn a_bare_user_is_a_person_with_no_roles() {
        let policy = Policy::default();
        let alice = user("alice@x.io");
        assert!(policy.allows(&alice, Permission::ThreadWrite, &thread_of(&alice)));
        assert!(!policy.allows(
            &alice,
            Permission::ThreadRead,
            &thread_of(&user("bob@x.io"))
        ));
        assert!(!policy.allows(&alice, Permission::Admin, &Resource::Anything));
        assert!(!Policy::deny_all().allows(&alice, Permission::ThreadRead, &thread_of(&alice)));
    }

    #[test]
    fn the_owner_is_compared_as_the_user_key() {
        // The key is the e-mail, trimmed and lower-cased by `UserId::new`.
        let policy = Policy::default();
        let me = principal("  Alice@X.io ", &["user"]);
        assert!(policy.allows(
            &me,
            Permission::ThreadWrite,
            &thread_of(&user("alice@x.io"))
        ));
        assert!(!policy.allows(
            &me,
            Permission::ThreadWrite,
            &thread_of(&user("alice@x.io.evil"))
        ));
    }

    #[test]
    fn roles_are_unioned_and_each_is_judged_alone() {
        let policy = Policy::new(
            BTreeMap::from([
                (
                    Role::new("reader"),
                    RoleGrant {
                        permissions: BTreeSet::from([
                            Permission::ThreadRead,
                            Permission::AgentRead,
                        ]),
                        agents: AgentScope::All,
                        max_tier: None,
                    },
                ),
                (
                    Role::new("coder-user"),
                    RoleGrant {
                        permissions: BTreeSet::from([
                            Permission::ThreadWrite,
                            Permission::AgentInvoke,
                        ]),
                        agents: AgentScope::from_patterns(["coder"]),
                        max_tier: None,
                    },
                ),
            ]),
            None,
        )
        .unwrap();
        let alice = user("alice@x.io");
        let bob = user("bob@x.io");
        let both = principal("alice@x.io", &["reader", "coder-user"]);
        let access = policy.access(&both);
        // Read from the first role, write from the second, both over the person's own threads.
        assert!(access.allows(Permission::ThreadRead, &thread_of(&alice)));
        assert!(!access.allows(Permission::ThreadRead, &thread_of(&bob)));
        assert!(access.allows(Permission::ThreadWrite, &thread_of(&alice)));
        assert!(!access.allows(Permission::ThreadWrite, &thread_of(&bob)));
        // Invoking is limited by the role that holds it; reading agents by the one that does.
        let coder = AgentId::new("coder");
        let chat = AgentId::new("chat");
        assert!(access.allows(
            Permission::AgentInvoke,
            &Resource::Agent {
                id: &coder,
                audience: &AgentAudience::Static
            }
        ));
        assert!(!access.allows(
            Permission::AgentInvoke,
            &Resource::Agent {
                id: &chat,
                audience: &AgentAudience::Static
            }
        ));
        assert!(access.allows(
            Permission::AgentRead,
            &Resource::Agent {
                id: &chat,
                audience: &AgentAudience::Static
            }
        ));
        // A role's agents do not leak into a permission it does not hold.
        assert_eq!(access.agents(Permission::AgentInvoke).patterns(), ["coder"]);
        assert_eq!(access.agents(Permission::AgentRead).patterns(), ["*"]);
        assert_eq!(
            access.agents(Permission::Admin),
            AgentScope::Only(BTreeSet::new())
        );
        assert_eq!(
            access.permissions(),
            [
                (Permission::AgentRead, None),
                (Permission::AgentInvoke, None),
                (Permission::ThreadRead, Some(Scope::Own)),
                (Permission::ThreadWrite, Some(Scope::Own)),
            ]
        );
        // Only the reader alone cannot write at all.
        let reader = principal("alice@x.io", &["reader"]);
        assert!(!policy.allows(&reader, Permission::ThreadWrite, &thread_of(&alice)));
    }

    #[test]
    fn every_scoped_permission_is_over_the_persons_own_threads_in_every_role() {
        let policy = Policy::default();
        let both = principal("root@x.io", &["user", "admin"]);
        let access = policy.access(&both);
        for permission in [
            Permission::ThreadRead,
            Permission::ThreadWrite,
            Permission::ArtifactRead,
        ] {
            assert_eq!(access.scope(permission), Some(Scope::Own), "{permission}");
        }
        for permission in [
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::Admin,
        ] {
            assert_eq!(access.scope(permission), None, "{permission}");
        }
        // A permission no role holds has no scope to speak of.
        let reader = Policy::new(
            BTreeMap::from([(
                Role::new("r"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::ThreadRead]),
                    ..RoleGrant::none()
                },
            )]),
            Some(Role::new("r")),
        )
        .unwrap();
        let access = reader.access(&both);
        assert_eq!(access.scope(Permission::ThreadRead), Some(Scope::Own));
        assert_eq!(access.scope(Permission::ThreadWrite), None);
        assert_eq!(access.roles().map(Role::as_str).collect::<Vec<_>>(), ["r"]);
    }

    #[test]
    fn a_role_that_names_no_agent_grants_no_agent_permission() {
        let mut grant = RoleGrant::user();
        grant.agents = AgentScope::from_patterns(Vec::<String>::new());
        let policy = Policy::new(
            BTreeMap::from([(Role::new("user"), grant)]),
            Some(Role::new("user")),
        )
        .unwrap();
        let alice = user("alice@x.io");
        let coder = AgentId::new("coder");
        assert!(!policy.allows(
            &alice,
            Permission::AgentInvoke,
            &Resource::Agent {
                id: &coder,
                audience: &AgentAudience::Static
            }
        ));
        assert!(!policy.allows(&alice, Permission::AgentInvoke, &Resource::Anything));
        // The thread permissions are not about agents.
        assert!(policy.allows(&alice, Permission::ThreadRead, &thread_of(&alice)));
    }

    #[test]
    fn check_tells_a_missing_permission_from_an_out_of_scope_resource() {
        let policy = Policy::default();
        let (alice, bob) = (user("alice@x.io"), user("bob@x.io"));
        let a = principal("alice@x.io", &["user"]);
        assert_eq!(
            policy.check(&a, Permission::ThreadRead, &thread_of(&alice)),
            Ok(())
        );
        assert_eq!(
            policy.check(&a, Permission::ThreadRead, &thread_of(&bob)),
            Err(Denied::OutOfScope(Permission::ThreadRead))
        );
        assert_eq!(
            policy.check(&a, Permission::Admin, &Resource::Anything),
            Err(Denied::Permission(Permission::Admin))
        );
        let coder = AgentId::new("coder");
        let only_chat = Policy::new(
            BTreeMap::from([(
                Role::new("chatter"),
                RoleGrant {
                    agents: AgentScope::from_patterns(["chat"]),
                    ..RoleGrant::user()
                },
            )]),
            Some(Role::new("chatter")),
        )
        .unwrap();
        assert_eq!(
            only_chat.check(
                &a,
                Permission::AgentInvoke,
                &Resource::Agent {
                    id: &coder,
                    audience: &AgentAudience::Static
                }
            ),
            Err(Denied::OutOfScope(Permission::AgentInvoke))
        );
    }

    #[test]
    fn a_default_role_must_be_defined() {
        assert_eq!(
            Policy::new(BTreeMap::new(), Some(Role::new("user"))).unwrap_err(),
            PolicyError::UnknownDefaultRole("user".to_owned())
        );
        assert!(Policy::new(BTreeMap::new(), None).is_ok());
        let policy = Policy::default();
        assert_eq!(policy.default_role().map(Role::as_str), Some("user"));
        assert_eq!(
            policy.role_names().map(Role::as_str).collect::<Vec<_>>(),
            ["admin", "user"]
        );
    }

    #[test]
    fn a_role_with_no_permissions_is_an_empty_access() {
        let policy = Policy::new(
            BTreeMap::from([(Role::new("nobody"), RoleGrant::none())]),
            Some(Role::new("nobody")),
        )
        .unwrap();
        let access = policy.access(&user("a@x.io"));
        assert!(access.is_empty());
        assert!(access.permissions().is_empty());
    }

    #[test]
    fn the_built_in_roles_are_the_adr_s() {
        let user = RoleGrant::user();
        assert!(!user.permissions.contains(&Permission::Admin));
        // The administrator is a user who also holds `admin`, and nothing more: no reach, no
        // permission over anyone's threads that a user lacks (ADR 0039).
        let admin = RoleGrant::admin();
        let mut expected = user.permissions.clone();
        expected.insert(Permission::Admin);
        assert_eq!(admin.permissions, expected);
        assert!(admin.permissions.contains(&Permission::Admin));
        assert_eq!(admin.agents, AgentScope::All);
    }

    // ---- an agent's audience (ADR 0045, decision 1) ----

    fn values<const N: usize>(values: [&str; N]) -> AgentAudience {
        AgentAudience::from_values(values)
    }

    /// May `who`, under `policy`, use `permission` on the agent `id` whose audience is `audience`?
    fn may(
        policy: &Policy,
        who: &Principal,
        permission: Permission,
        id: &str,
        audience: &AgentAudience,
    ) -> bool {
        let id = AgentId::new(id);
        policy.allows(who, permission, &Resource::Agent { id: &id, audience })
    }

    #[test]
    fn a_static_agent_has_no_audience_and_the_roles_alone_decide() {
        let policy = Policy::default();
        let id = AgentId::new("coder");
        for who in [
            principal("a@x.io", &["user"]),
            principal("a@x.io", &[]),
            principal("a@x.io", &["something-unknown"]),
            principal("a@x.io", &["admin"]),
        ] {
            for permission in [Permission::AgentRead, Permission::AgentInvoke] {
                assert!(
                    policy.allows(
                        &who,
                        permission,
                        &Resource::Agent {
                            id: &id,
                            audience: &AgentAudience::Static
                        }
                    ),
                    "{who:?} {permission}"
                );
            }
        }
        // and the roles still say no where they say no
        let only_chat = Policy::new(
            BTreeMap::from([(
                Role::new("chatter"),
                RoleGrant {
                    agents: AgentScope::from_patterns(["chat"]),
                    ..RoleGrant::user()
                },
            )]),
            None,
        )
        .unwrap();
        let chatter = principal("a@x.io", &["chatter"]);
        assert!(!may(
            &only_chat,
            &chatter,
            Permission::AgentInvoke,
            "coder",
            &AgentAudience::Static
        ));
    }

    #[test]
    fn a_registry_agent_with_none_is_for_administrators_only() {
        let policy = Policy::default();
        let none = AgentAudience::AdminsOnly;
        for permission in [Permission::AgentRead, Permission::AgentInvoke] {
            assert!(
                !may(
                    &policy,
                    &principal("a@x.io", &["user"]),
                    permission,
                    "new",
                    &none
                ),
                "a user"
            );
            assert!(
                !may(&policy, &principal("a@x.io", &[]), permission, "new", &none),
                "a person with no roles, who is a user by default"
            );
            assert!(
                !may(
                    &policy,
                    &principal("a@x.io", &["agent.use:new"]),
                    permission,
                    "new",
                    &none
                ),
                "a use permission is a value of an audience, and there is none"
            );
            assert!(
                may(
                    &policy,
                    &principal("a@x.io", &["admin"]),
                    permission,
                    "new",
                    &none
                ),
                "an administrator"
            );
        }
    }

    #[test]
    fn star_admits_everyone_the_roles_let_in() {
        let policy = Policy::default();
        for who in [
            principal("a@x.io", &["user"]),
            principal("a@x.io", &[]),
            principal("a@x.io", &["admin"]),
        ] {
            assert!(may(
                &policy,
                &who,
                Permission::AgentInvoke,
                "chat",
                &AgentAudience::Everyone
            ));
            assert!(may(
                &policy,
                &who,
                Permission::AgentRead,
                "chat",
                &AgentAudience::Everyone
            ));
        }
    }

    #[test]
    fn values_are_met_by_one_of_the_roles_claim_known_to_the_policy_or_not() {
        let policy = Policy::default();
        let some = values(["agent.use:coder-me", "agent.use:coder-vymalo"]);
        let who = |roles: &[&str]| principal("a@x.io", roles);
        // the claim holds one of the values: `agent.use:coder-me` is a role of the identity
        // provider that `auth.roles` knows nothing of
        assert!(may(
            &policy,
            &who(&["user", "agent.use:coder-me"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        assert!(may(
            &policy,
            &who(&["agent.use:coder-vymalo"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        // none of them
        assert!(!may(
            &policy,
            &who(&["user"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        assert!(!may(
            &policy,
            &who(&[]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        assert!(!may(
            &policy,
            &who(&["user", "agent.use:chat"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        // byte for byte: no case folding, no trimming, no prefix
        assert!(!may(
            &policy,
            &who(&["user", "Agent.Use:Coder-Me"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        assert!(!may(
            &policy,
            &who(&["user", "agent.use:coder-me "]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        assert!(!may(
            &policy,
            &who(&["user", "agent.use:coder"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
        // an administrator needs no value
        assert!(may(
            &policy,
            &who(&["admin"]),
            Permission::AgentInvoke,
            "coder-me",
            &some
        ));
    }

    #[test]
    fn a_person_who_has_no_role_grant_is_refused_whatever_the_audience() {
        // `agent.use:x` is in the claim and in the audience, and no role of the policy is known
        // and there is no default role: (a) fails, and (b) cannot make up for it
        let policy = Policy::new(
            BTreeMap::from([(Role::new("user"), RoleGrant::user())]),
            None,
        )
        .unwrap();
        let stranger = principal("a@x.io", &["agent.use:x"]);
        let audiences = [
            AgentAudience::Everyone,
            AgentAudience::AdminsOnly,
            values(["agent.use:x"]),
            AgentAudience::Static,
        ];
        for audience in &audiences {
            for permission in [Permission::AgentRead, Permission::AgentInvoke] {
                assert!(
                    !may(&policy, &stranger, permission, "x", audience),
                    "{audience:?}"
                );
                let id = AgentId::new("x");
                assert_eq!(
                    policy
                        .access(&stranger)
                        .check(permission, &Resource::Agent { id: &id, audience }),
                    Err(Denied::Permission(permission)),
                    "{audience:?}"
                );
            }
        }
        // a role that grants the permission over other agents only: out of scope, audience or not
        let only_chat = Policy::new(
            BTreeMap::from([
                (
                    Role::new("chatter"),
                    RoleGrant {
                        agents: AgentScope::from_patterns(["chat"]),
                        ..RoleGrant::user()
                    },
                ),
                (Role::new("admin"), RoleGrant::admin()),
            ]),
            None,
        )
        .unwrap();
        let chatter = principal("a@x.io", &["chatter", "agent.use:x"]);
        assert!(!may(
            &only_chat,
            &chatter,
            Permission::AgentInvoke,
            "x",
            &values(["agent.use:x"])
        ));
        let id = AgentId::new("x");
        assert_eq!(
            only_chat.access(&chatter).check(
                Permission::AgentInvoke,
                &Resource::Agent {
                    id: &id,
                    audience: &values(["agent.use:x"])
                }
            ),
            Err(Denied::OutOfScope(Permission::AgentInvoke))
        );
    }

    #[test]
    fn an_administrator_passes_the_audience_and_still_needs_a_role_over_the_agent() {
        // `ops` holds `admin` and `agent.invoke` over `chat` only
        let policy = Policy::new(
            BTreeMap::from([(
                Role::new("ops"),
                RoleGrant {
                    agents: AgentScope::from_patterns(["chat"]),
                    ..RoleGrant::admin()
                },
            )]),
            None,
        )
        .unwrap();
        let ops = principal("a@x.io", &["ops"]);
        // (b) by `admin`, (a) over `chat`
        assert!(may(
            &policy,
            &ops,
            Permission::AgentInvoke,
            "chat",
            &AgentAudience::AdminsOnly
        ));
        assert!(may(
            &policy,
            &ops,
            Permission::AgentInvoke,
            "chat",
            &values(["agent.use:chat"])
        ));
        // (b) by `admin`, but (a) is not over `coder`
        assert!(!may(
            &policy,
            &ops,
            Permission::AgentInvoke,
            "coder",
            &AgentAudience::AdminsOnly
        ));
        assert!(!may(
            &policy,
            &ops,
            Permission::AgentRead,
            "coder",
            &AgentAudience::Everyone
        ));
        // `admin` held by one role and the grant over the agent by another is still an administrator
        let split = Policy::new(
            BTreeMap::from([
                (
                    Role::new("watch"),
                    RoleGrant {
                        permissions: BTreeSet::from([Permission::Admin]),
                        max_tier: None,
                        agents: AgentScope::Only(BTreeSet::new()),
                    },
                ),
                (
                    Role::new("chatter"),
                    RoleGrant {
                        agents: AgentScope::from_patterns(["chat"]),
                        ..RoleGrant::user()
                    },
                ),
            ]),
            None,
        )
        .unwrap();
        let both = principal("a@x.io", &["watch", "chatter"]);
        assert!(may(
            &split,
            &both,
            Permission::AgentInvoke,
            "chat",
            &AgentAudience::AdminsOnly
        ));
        let chatter_only = principal("a@x.io", &["chatter"]);
        assert!(!may(
            &split,
            &chatter_only,
            Permission::AgentInvoke,
            "chat",
            &AgentAudience::AdminsOnly
        ));
    }

    #[test]
    fn check_tells_the_audience_from_the_roles_and_check_scope_leaves_it_out() {
        let policy = Policy::default();
        let id = AgentId::new("new");
        let alice = principal("a@x.io", &["user"]);
        let access = policy.access(&alice);
        let none = AgentAudience::AdminsOnly;
        assert_eq!(
            access.check(
                Permission::AgentInvoke,
                &Resource::Agent {
                    id: &id,
                    audience: &none
                }
            ),
            Err(Denied::Audience(Permission::AgentInvoke))
        );
        assert_eq!(
            access.check(
                Permission::AgentRead,
                &Resource::Agent {
                    id: &id,
                    audience: &none
                }
            ),
            Err(Denied::Audience(Permission::AgentRead))
        );
        // the roles' half alone: yes, which is why it must never be the last word
        assert_eq!(access.check_scope(Permission::AgentInvoke, &id), Ok(()));
        assert!(access.meets_audience(&AgentAudience::Everyone));
        assert!(!access.meets_audience(&none));
        let root = principal("r@x.io", &["admin"]);
        assert_eq!(
            policy.access(&root).check(
                Permission::AgentInvoke,
                &Resource::Agent {
                    id: &id,
                    audience: &none
                }
            ),
            Ok(())
        );
    }

    #[test]
    fn an_audience_is_about_agents_and_no_other_permission_or_resource() {
        let policy = Policy::default();
        let alice = principal("a@x.io", &["user"]);
        let owner = user("a@x.io");
        let none = AgentAudience::AdminsOnly;
        let id = AgentId::new("new");
        // a thread is the person's whatever the audience of the agent it talks to
        assert!(policy.allows(&alice, Permission::ThreadRead, &thread_of(&owner)));
        // and a permission over threads asked of an agent is still no
        assert!(!policy.allows(
            &alice,
            Permission::ThreadRead,
            &Resource::Agent {
                id: &id,
                audience: &AgentAudience::Everyone
            }
        ));
        assert!(!policy.allows(
            &alice,
            Permission::ThreadWrite,
            &Resource::Agent {
                id: &id,
                audience: &none
            }
        ));
        // the permission itself, for the listing, is not an agent's
        assert!(policy.allows(&alice, Permission::AgentInvoke, &Resource::Anything));
    }

    #[test]
    fn somebody_could_invoke_an_agent_when_a_role_may_and_its_audience_can_let_somebody_in() {
        let user_only = Policy::new(
            BTreeMap::from([(Role::new("user"), RoleGrant::user())]),
            Some(Role::new("user")),
        )
        .unwrap();
        let with_admin = Policy::default();
        let coder = AgentId::new("coder");
        let everyone = AgentAudience::Everyone;
        let some = values(["agent.use:coder"]);
        let none = AgentAudience::AdminsOnly;
        for audience in [&AgentAudience::Static, &everyone, &some] {
            assert!(
                user_only.any_role_may_invoke(&coder, audience),
                "{audience:?}"
            );
            assert!(
                with_admin.any_role_may_invoke(&coder, audience),
                "{audience:?}"
            );
        }
        // none: only a deployment with an administrator can have anybody in
        assert!(!user_only.any_role_may_invoke(&coder, &none));
        assert!(with_admin.any_role_may_invoke(&coder, &none));
        // and the roles' side still decides first
        let only_chat = Policy::new(
            BTreeMap::from([
                (
                    Role::new("chatter"),
                    RoleGrant {
                        agents: AgentScope::from_patterns(["chat"]),
                        ..RoleGrant::user()
                    },
                ),
                (
                    Role::new("admin"),
                    RoleGrant {
                        permissions: BTreeSet::from([Permission::Admin]),
                        max_tier: None,
                        agents: AgentScope::Only(BTreeSet::new()),
                    },
                ),
            ]),
            None,
        )
        .unwrap();
        assert!(!only_chat.any_role_may_invoke(&coder, &none));
        assert!(!only_chat.any_role_may_invoke(&coder, &everyone));
        assert!(only_chat.any_role_may_invoke(&AgentId::new("chat"), &none));
    }

    mod audience_properties {
        use proptest::prelude::*;

        use super::*;

        const NAMES: [&str; 5] = ["a", "b", "c", "admin", "user"];

        /// An audience made of the same few names the claims are, so that a match is likely.
        fn audience() -> impl Strategy<Value = AgentAudience> {
            prop_oneof![
                Just(AgentAudience::Static),
                Just(AgentAudience::Everyone),
                Just(AgentAudience::AdminsOnly),
                prop::collection::vec(
                    prop::sample::select(vec!["a", "b", "c", "admin", "user", "*", "z"]),
                    0..5
                )
                .prop_map(AgentAudience::from_values),
            ]
        }

        fn claims() -> impl Strategy<Value = Vec<&'static str>> {
            prop::collection::vec(prop::sample::select(NAMES.to_vec()), 0..5)
        }

        /// The rule of ADR 0045 decision 1.4, written out once more, independently.
        fn model(granted: bool, is_admin: bool, claims: &[&str], audience: &AgentAudience) -> bool {
            let met = match audience {
                AgentAudience::Static | AgentAudience::Everyone => true,
                AgentAudience::AdminsOnly => is_admin,
                AgentAudience::Roles(wanted) => {
                    is_admin || claims.iter().any(|c| wanted.contains(c))
                }
            };
            granted && met
        }

        /// A policy where `a` grants both agent permissions over `x`, `b` over `y`, `admin` holds
        /// `admin` and everything over every agent; the rest of the names are unknown.
        fn policy() -> Policy {
            Policy::new(
                BTreeMap::from([
                    (
                        Role::new("a"),
                        RoleGrant {
                            agents: AgentScope::from_patterns(["x"]),
                            ..RoleGrant::user()
                        },
                    ),
                    (
                        Role::new("b"),
                        RoleGrant {
                            agents: AgentScope::from_patterns(["y"]),
                            ..RoleGrant::user()
                        },
                    ),
                    (Role::new("admin"), RoleGrant::admin()),
                ]),
                None,
            )
            .unwrap()
        }

        proptest! {
            #[test]
            fn the_rule_is_both_halves(claims in claims(), audience in audience(), on_y in any::<bool>()) {
                let policy = policy();
                let who = principal("p@x.io", &claims);
                let agent = if on_y { "y" } else { "x" };
                let granted = claims.contains(&"admin")
                    || claims.contains(&if on_y { "b" } else { "a" });
                let is_admin = claims.contains(&"admin");
                for permission in [Permission::AgentRead, Permission::AgentInvoke] {
                    prop_assert_eq!(
                        may(&policy, &who, permission, agent, &audience),
                        model(granted, is_admin, &claims, &audience),
                        "{:?} {} {:?}", claims, agent, audience
                    );
                }
            }

            #[test]
            fn an_audience_never_grants_what_the_roles_do_not(claims in claims(), audience in audience()) {
                let policy = policy();
                let who = principal("p@x.io", &claims);
                let id = AgentId::new("x");
                let access = policy.access(&who);
                let by_scope = access.check_scope(Permission::AgentInvoke, &id);
                let full = access.check(Permission::AgentInvoke, &Resource::Agent { id: &id, audience: &audience });
                // the audience can refuse what the roles allow, and never allow what they refuse
                if by_scope.is_err() {
                    prop_assert!(full.is_err());
                    prop_assert!(!matches!(full, Err(Denied::Audience(_))), "the roles refuse first");
                }
                if full.is_ok() {
                    prop_assert!(by_scope.is_ok());
                }
            }

            #[test]
            fn more_roles_in_the_claim_never_lose_access(
                claims in claims(), more in claims(), audience in audience()
            ) {
                // only the audience's half is monotone: a role can also take the policy's
                // default away, so ask the audience alone
                let few = principal("p@x.io", &claims);
                let mut all = claims.clone();
                all.extend(more);
                let many = principal("p@x.io", &all);
                let policy = policy();
                if policy.access(&few).meets_audience(&audience) {
                    prop_assert!(policy.access(&many).meets_audience(&audience));
                }
            }

            #[test]
            fn everyone_and_static_admit_all_and_the_administrator_is_admitted_by_all(
                claims in claims(), audience in audience()
            ) {
                let policy = policy();
                let access = policy.access(&principal("p@x.io", &claims));
                if matches!(audience, AgentAudience::Everyone | AgentAudience::Static) {
                    prop_assert!(access.meets_audience(&audience));
                }
                if claims.contains(&"admin") {
                    prop_assert!(access.meets_audience(&audience));
                }
            }
        }
    }
}
