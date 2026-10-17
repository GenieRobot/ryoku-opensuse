//! Keep the installed privilege boundary aligned with the Rust helper policy.

const HELPER_SERVICE: &str = include_str!("../../../packaging/rpm/systemd/ryoku-helper.service");
const HELPER_SOCKET: &str = include_str!("../../../packaging/rpm/systemd/ryoku-helper.socket");
const USER_SERVICE: &str =
    include_str!("../../../packaging/rpm/systemd/user/ryoku-service.service");
const SYSUSERS: &str = include_str!("../../../packaging/rpm/sysusers.d/ryoku.conf");
const RPM_SPEC: &str = include_str!("../../../packaging/rpm/ryoku-helper.spec");

fn section<'a>(unit: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("[{name}]");
    let mut current = false;
    let mut lines = Vec::new();
    for line in unit.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            current = line == header;
        } else if current && !line.is_empty() && !line.starts_with('#') {
            lines.push(line);
        }
    }
    lines
}

fn has_directive(lines: &[&str], expected: &str) -> bool {
    lines.iter().any(|line| *line == expected)
}

fn rpm_section<'a>(spec: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("%{name}");
    let mut current = false;
    let mut lines = Vec::new();
    for line in spec.lines().map(str::trim) {
        if line == header {
            current = true;
        } else if current
            && line.starts_with('%')
            && !line.starts_with("%attr(")
            && !line.starts_with("%dir ")
            && !line.starts_with("%license ")
            && !line.starts_with("%doc ")
        {
            current = false;
        } else if current && !line.is_empty() && !line.starts_with('#') {
            lines.push(line);
        }
    }
    lines
}

#[test]
fn helper_is_an_explicit_root_scoped_deny_only_service() {
    let service = section(HELPER_SERVICE, "Service");
    assert!(has_directive(&service, "User=root"));
    assert!(has_directive(&service, "Group=root"));
    assert!(has_directive(&service, "NoNewPrivileges=yes"));
    assert!(has_directive(&service, "CapabilityBoundingSet="));
    assert!(has_directive(&service, "AmbientCapabilities="));
    assert!(has_directive(&service, "ProtectSystem=strict"));
    assert!(has_directive(&service, "ProtectHome=read-only"));
    assert!(has_directive(&service, "RestrictAddressFamilies=AF_UNIX"));
    assert!(has_directive(&service, "ExecStart=/usr/libexec/ryoku/ryoku-helper"));
    assert!(section(HELPER_SERVICE, "Install").is_empty());
}

#[test]
fn helper_socket_is_fixed_root_owned_and_opt_in_group_only() {
    let socket = section(HELPER_SOCKET, "Socket");
    assert!(has_directive(&socket, "ListenStream=/run/ryoku/helper.sock"));
    assert!(has_directive(&socket, "SocketUser=root"));
    assert!(has_directive(&socket, "SocketGroup=ryoku-operators"));
    assert!(has_directive(&socket, "SocketMode=0660"));
    assert!(has_directive(&socket, "DirectoryMode=0755"));
    assert!(has_directive(&socket, "Accept=no"));

    let group_declarations: Vec<_> = SYSUSERS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(group_declarations, ["g ryoku-operators - -"]);
}

#[test]
fn package_installs_units_without_enabling_the_privileged_socket() {
    let files = rpm_section(RPM_SPEC, "files");
    assert!(files.iter().any(|line| line.ends_with("/ryoku-helper.service")));
    assert!(files.iter().any(|line| line.ends_with("/ryoku-helper.socket")));
    assert!(files.iter().any(|line| line.ends_with("/ryoku.conf")));
    assert!(files.iter().any(|line| line.ends_with("/ryoku-helper")));
    assert!(
        section(HELPER_SOCKET, "Install").iter().any(|line| { *line == "WantedBy=sockets.target" })
    );
    assert!(
        section(USER_SERVICE, "Install").iter().any(|line| { *line == "WantedBy=default.target" })
    );
    assert!(USER_SERVICE.contains("Environment=XDG_STATE_HOME=%h/.local/state"));
}
