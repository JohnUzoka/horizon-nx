//! Physical process information, frozen before any mod is loaded.
use horizon_mod_package::selection::{Argument, LaunchArguments};
use mod_api::{Dist, RuntimeEnvironment};
use std::sync::OnceLock;

static ENVIRONMENT: OnceLock<RuntimeEnvironment> = OnceLock::new();

/// Rust has no `current_exe` on Horizon. The homebrew loader passes the NRO path
/// as argv[0]; fall back to the working directory if that path was not supplied.
pub(crate) fn executable_dir() -> crate::Result<std::path::PathBuf> {
    #[cfg(target_os = "horizon")]
    let dir = horizon_nx::nro_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    #[cfg(not(target_os = "horizon"))]
    let dir = std::env::current_exe()?
        .parent()
        .ok_or("missing executable directory")?
        .to_path_buf();
    Ok(dir)
}

#[cfg(target_os = "horizon")]
pub(super) fn switch_launch_arguments() -> crate::Result<LaunchArguments> {
    let dir = executable_dir()?;
    let mut file_args = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("args.txt")) {
        for line in text.lines() {
            let args = line.split_once('#').map_or(line, |(args, _)| args);
            file_args.extend(args.split_whitespace().map(str::to_owned));
        }
    }
    if !file_args.is_empty() {
        // The game logger is initialized after validation. libnx redirects stderr
        // to the Switch log already, including for an invalid command line.
        eprintln!("Horizon: args.txt next to the NRO supplied: {}", file_args.join(" "));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| dir.clone());
    let mut launch = LaunchArguments::parse(
        std::env::args().skip(1).chain(file_args),
        &cwd,
        &dir,
        std::env::var_os("HORIZON_HOME").as_deref(),
    )?;
    // Use the same parsed argument as explicit --dedicated so installation and
    // hook dispatch see the server role before the game options are validated.
    if !dir.join("client.txt").exists()
        && !launch.is_information_request()
        && launch.installation_side() != horizon_mod_package::manifest::Side::Server
    {
        launch.rest.push(Argument::Dedicated(Vec::new()));
    }
    Ok(launch)
}

fn snapshot(launch: &LaunchArguments, enabled: Option<&str>) -> RuntimeEnvironment {
    let dedicated = launch
        .rest
        .iter()
        .any(|arg| matches!(arg, Argument::Dedicated(_)));
    let flag = launch.rest.iter().any(|arg| match arg {
        Argument::Switch(name) => name == "--game-tests",
        Argument::Dedicated(args) => super::dedicated_arguments::parse(args)
            .any(|arg| matches!(arg, Ok(("game-tests", None)))),
        _ => false,
    });
    RuntimeEnvironment::new(
        if dedicated {
            Dist::DedicatedServer
        } else {
            Dist::Client
        },
        horizon_core::VANILLA_VERSION.to_owned(),
        crate::crash::version(),
        flag || matches!(enabled, Some("1" | "true")),
    )
}

pub(crate) fn capture(launch: &LaunchArguments) {
    ENVIRONMENT
        .set(snapshot(
            launch,
            std::env::var("HORIZON_GAME_TESTS").ok().as_deref(),
        ))
        .expect("process runtime environment captured once");
}

pub(crate) fn get() -> &'static RuntimeEnvironment {
    ENVIRONMENT
        .get()
        .expect("process runtime environment captured before registration")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dedicated_tail_accepts_game_tests() {
        let args = ["--dedicated", "--game-tests"].map(str::to_owned);
        let launch = LaunchArguments::parse(
            args.into_iter(),
            std::path::Path::new("/"),
            std::path::Path::new("/"),
            None,
        )
        .unwrap();
        let info = snapshot(&launch, None);
        assert_eq!(info.dist(), Dist::DedicatedServer);
        assert!(info.game_tests_enabled());
    }

    #[test]
    fn physical_process_versions_and_test_opt_in() {
        for dedicated in [false, true] {
            for flag in [false, true] {
                let mut args = Vec::new();
                if flag {
                    args.push("--game-tests".to_owned());
                }
                if dedicated {
                    args.push("--dedicated".to_owned());
                }
                let launch = LaunchArguments::parse(
                    args.into_iter(),
                    std::path::Path::new("/"),
                    std::path::Path::new("/"),
                    None,
                )
                .unwrap();
                for env in [None, Some("0"), Some("false"), Some("1"), Some("true")] {
                    let info = snapshot(&launch, env);
                    assert_eq!(
                        info.dist(),
                        if dedicated {
                            Dist::DedicatedServer
                        } else {
                            Dist::Client
                        }
                    );
                    assert_eq!(info.game_version(), horizon_core::VANILLA_VERSION);
                    assert_eq!(info.horizon_version(), crate::crash::version());
                    assert_eq!(
                        info.game_tests_enabled(),
                        flag || matches!(env, Some("1" | "true"))
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    #[test]
    fn game_test_flags_do_not_steal_option_operands() {
        for option in ["world", "universe", "port", "serverId", "pidFile"] {
            for before in [false, true] {
                for after in [false, true] {
                    let mut args = Vec::new();
                    if before {
                        args.push("--game-tests".to_owned());
                    }
                    args.extend([
                        "--dedicated".to_owned(),
                        format!("--{option}"),
                        "--game-tests".to_owned(),
                    ]);
                    if after {
                        args.push("--game-tests".to_owned());
                    }
                    let launch = LaunchArguments::parse(
                        args.into_iter(),
                        std::path::Path::new("/"),
                        std::path::Path::new("/"),
                        None,
                    )
                    .unwrap();
                    let info = snapshot(&launch, None);
                    assert_eq!(info.dist(), Dist::DedicatedServer);
                    assert_eq!(info.game_tests_enabled(), before || after, "{option}");
                }
            }
        }
        let launch = LaunchArguments::parse(
            ["--world", "--game-tests"].map(str::to_owned).into_iter(),
            std::path::Path::new("/"),
            std::path::Path::new("/"),
            None,
        )
        .unwrap();
        assert!(!snapshot(&launch, None).game_tests_enabled());
    }
}
