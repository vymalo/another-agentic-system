//! Roles, permissions and the one question the application asks of them: may this person do this
//! to that (ADR 0033).
//!
//! The model is pure: no I/O, no clock, no ports. A [`Policy`] maps role names, as an identity
//! provider spells them (a group, a realm role), to a [`RoleGrant`]: the [`Permission`]s the role
//! holds, how far they reach over other people's threads ([`Scope`]) and over which agents. A
//! person's [`Access`] is the union of the grants of the roles they carry, and
//! [`Policy::allows`] / [`Policy::check`] answer for one [`Resource`].
//!
//! **Fail closed.** A role the policy does not name grants nothing. A person none of whose roles
//! is named gets the policy's *default role* (`auth.defaultRole`), and nothing at all when there is
//! none. A permission over a resource of another kind (`thread.read` asked of an agent) is
//! denied.
//!
//! **Own and any.** A permission over threads has a scope. `own` reaches the threads the person
//! owns (the owner is the e-mail, [`Principal::user`]); `any` reaches every thread. The scope of a
//! role is given for reading (`thread.read`, `artifact.read`) and for writing (`thread.write`)
//! separately, which is how an administrator reads every thread and acts only on their own (owner
//! decision 4 of plan 10).
//!
//! Two answers, because they are told apart on the wire: a person whose roles do not hold a
//! permission at all is [`Denied::Permission`] (403: the answer does not depend on what is asked
//! for, so it leaks nothing), and one who holds it but not over this resource is
//! [`Denied::OutOfScope`] (a thread is then 404 when the person may not read it, so its existence
//! never leaks).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use orch_core::{AgentId, UserId};
use orch_ports::{Principal, Role};

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
    /// `artifact.read`: download the files of the threads the person may read.
    ArtifactRead,
    /// `admin`: ask for another person's threads (`GET /api/threads?owner=`).
    Admin,
}

impl Permission {
    /// Every permission, in the order they are listed.
    pub const ALL: [Permission; 6] = [
        Permission::AgentRead,
        Permission::AgentInvoke,
        Permission::ThreadRead,
        Permission::ThreadWrite,
        Permission::ArtifactRead,
        Permission::Admin,
    ];

    /// The name used in the configuration and by `GET /api/me`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::AgentRead => "agent.read",
            Permission::AgentInvoke => "agent.invoke",
            Permission::ThreadRead => "thread.read",
            Permission::ThreadWrite => "thread.write",
            Permission::ArtifactRead => "artifact.read",
            Permission::Admin => "admin",
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

/// How far a permission over threads reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// The threads the person owns.
    Own,
    /// Every thread.
    Any,
}

impl Scope {
    /// The name used in the configuration and by `GET /api/me`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::Own => "own",
            Scope::Any => "any",
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
    /// How far `thread.read` and `artifact.read` reach.
    pub read: Scope,
    /// How far `thread.write` reaches.
    pub write: Scope,
    /// The agents `agent.read` and `agent.invoke` are about.
    pub agents: AgentScope,
}

impl RoleGrant {
    /// A role that grants nothing.
    pub fn none() -> RoleGrant {
        RoleGrant {
            permissions: BTreeSet::new(),
            read: Scope::Own,
            write: Scope::Own,
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
                Permission::ArtifactRead,
            ]),
            read: Scope::Own,
            write: Scope::Own,
            agents: AgentScope::All,
        }
    }

    /// The administrator: a user who also holds `admin` and **reads** every thread, but changes
    /// only their own (owner decision 4 of plan 10).
    pub fn admin() -> RoleGrant {
        let mut grant = RoleGrant::user();
        grant.permissions.insert(Permission::Admin);
        grant.read = Scope::Any;
        grant
    }

    fn holds(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// The scope of a scoped permission, `None` for one that has none.
    fn scope_of(&self, permission: Permission) -> Option<Scope> {
        match permission {
            Permission::ThreadRead | Permission::ArtifactRead => Some(self.read),
            Permission::ThreadWrite => Some(self.write),
            Permission::AgentRead | Permission::AgentInvoke | Permission::Admin => None,
        }
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
    /// An agent.
    Agent {
        /// The agent's id.
        id: &'a AgentId,
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
    /// The roles hold the permission, not over this resource: another person's thread under a
    /// scope of `own`, or an agent a role's `agents` do not name.
    OutOfScope(Permission),
}

/// What one person may do: the grants of their known roles. Built by [`Policy::access`].
#[derive(Debug, Clone)]
pub struct Access<'p> {
    user: UserId,
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

    /// The widest scope a role gives `permission`; `None` when no role holds it or it has no
    /// scope (see [`Permission::is_scoped`]).
    pub fn scope(&self, permission: Permission) -> Option<Scope> {
        self.roles
            .iter()
            .filter(|(_, g)| g.holds(permission))
            .filter_map(|(_, g)| g.scope_of(permission))
            .max()
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
    pub fn allows(&self, permission: Permission, resource: &Resource<'_>) -> bool {
        self.roles
            .iter()
            .any(|(_, grant)| self.grant_allows(grant, permission, resource))
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
            ) => match grant.scope_of(permission) {
                Some(Scope::Any) => true,
                Some(Scope::Own) => **owner == self.user,
                None => false,
            },
            // Starting a thread is not about anyone's: it is a thread of the person's own.
            (Permission::ThreadWrite, Resource::Anything) => true,
            (Permission::AgentRead | Permission::AgentInvoke, Resource::Agent { id }) => {
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
            // A permission asked of a resource it is not about.
            _ => false,
        }
    }

    /// [`allows`](Self::allows), saying why not.
    ///
    /// # Errors
    /// [`Denied::Permission`] when no role holds `permission`, and [`Denied::OutOfScope`] when
    /// some does but none over `resource`.
    pub fn check(&self, permission: Permission, resource: &Resource<'_>) -> Result<(), Denied> {
        if !self.has(permission) {
            Err(Denied::Permission(permission))
        } else if self.allows(permission, resource) {
            Ok(())
        } else {
            Err(Denied::OutOfScope(permission))
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
    // administrator. Every row is one decision of the default policy.
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
            // The administrator reads everything, and acts only on their own.
            (&root, Permission::ThreadRead, &alice, true),
            (&root, Permission::ThreadRead, &user("root@x.io"), true),
            (&root, Permission::ThreadWrite, &alice, false),
            (&root, Permission::ThreadWrite, &user("root@x.io"), true),
            (&root, Permission::ArtifactRead, &bob, true),
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
                Resource::Agent { id: &coder },
                Resource::Anything,
            ] {
                let expected = matches!(
                    (permission, &resource),
                    (
                        Permission::ThreadRead | Permission::ThreadWrite | Permission::ArtifactRead,
                        Resource::Thread { .. }
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
                        read: Scope::Any,
                        write: Scope::Own,
                        agents: AgentScope::All,
                    },
                ),
                (
                    Role::new("coder-user"),
                    RoleGrant {
                        permissions: BTreeSet::from([
                            Permission::ThreadWrite,
                            Permission::AgentInvoke,
                        ]),
                        read: Scope::Own,
                        write: Scope::Own,
                        agents: AgentScope::from_patterns(["coder"]),
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
        // Read from the first role, write from the second.
        assert!(access.allows(Permission::ThreadRead, &thread_of(&bob)));
        assert!(access.allows(Permission::ThreadWrite, &thread_of(&alice)));
        assert!(!access.allows(Permission::ThreadWrite, &thread_of(&bob)));
        // Invoking is limited by the role that holds it; reading agents by the one that does.
        let coder = AgentId::new("coder");
        let chat = AgentId::new("chat");
        assert!(access.allows(Permission::AgentInvoke, &Resource::Agent { id: &coder }));
        assert!(!access.allows(Permission::AgentInvoke, &Resource::Agent { id: &chat }));
        assert!(access.allows(Permission::AgentRead, &Resource::Agent { id: &chat }));
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
                (Permission::ThreadRead, Some(Scope::Any)),
                (Permission::ThreadWrite, Some(Scope::Own)),
            ]
        );
        // Only the reader alone cannot write at all.
        let reader = principal("alice@x.io", &["reader"]);
        assert!(!policy.allows(&reader, Permission::ThreadWrite, &thread_of(&alice)));
    }

    #[test]
    fn the_widest_scope_wins_across_roles() {
        let policy = Policy::default();
        let both = principal("root@x.io", &["user", "admin"]);
        let access = policy.access(&both);
        assert_eq!(access.scope(Permission::ThreadRead), Some(Scope::Any));
        assert_eq!(access.scope(Permission::ThreadWrite), Some(Scope::Own));
        assert_eq!(access.scope(Permission::Admin), None);
        assert_eq!(
            access.roles().map(Role::as_str).collect::<Vec<_>>(),
            ["admin", "user"]
        );
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
            &Resource::Agent { id: &coder }
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
            only_chat.check(&a, Permission::AgentInvoke, &Resource::Agent { id: &coder }),
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
        assert_eq!(user.read, Scope::Own);
        assert_eq!(user.write, Scope::Own);
        assert!(!user.permissions.contains(&Permission::Admin));
        let admin = RoleGrant::admin();
        assert_eq!((admin.read, admin.write), (Scope::Any, Scope::Own));
        assert!(admin.permissions.contains(&Permission::Admin));
        assert_eq!(admin.agents, AgentScope::All);
    }
}
