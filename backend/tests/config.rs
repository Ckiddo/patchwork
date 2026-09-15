use backend::config::Config;

#[test]
fn config_errors_do_not_include_source_secrets_and_relative_paths_are_stable() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("server.toml");
    std::fs::write(&file, "jwt_secret = 'sensitive-source-marker'\n").unwrap();
    assert!(matches!(Config::load(&file), Err("invalid config file")));
    let valid = include_str!("../config.example.toml");
    std::fs::write(&file, valid).unwrap();
    let config = Config::load(&file).unwrap();
    assert_eq!(config.jwt_secret_file, dir.path().join("secrets/jwt.key"));
    assert!(config.load_jwt_secret().is_err());
    std::fs::write(&file, valid.replace("workers = 2", "workers = 0")).unwrap();
    assert!(Config::load(&file).is_err());
    std::fs::write(
        &file,
        valid.replace(
            "https://ckiddo.github.io",
            "https://ckiddo.github.io/patchwork/",
        ),
    )
    .unwrap();
    assert!(Config::load(&file).is_err());
}

#[test]
fn database_configuration_is_local_bounded_and_resolves_secret_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("db.toml");
    let source = format!(
        "{}\n[database]\nhost='127.0.0.1'\nport=15432\nname='patchwork_dev'\nuser='patchwork_app'\npassword_file='secrets/db.key'\n",
        include_str!("../config.example.toml")
    );
    std::fs::write(&file, &source).unwrap();
    let db = Config::load(&file).unwrap().database.unwrap();
    assert_eq!(db.password_file, dir.path().join("secrets/db.key"));
    assert_eq!(db.max_connections, 8);
    assert!(db.options().is_err());
    for invalid in [
        source.replace("host='127.0.0.1'", "host='192.168.5.9'"),
        format!("{source}\nmax_connections=0\n"),
        format!("{source}\nlock_timeout_ms=0\n"),
    ] {
        std::fs::write(&file, invalid).unwrap();
        assert!(Config::load(&file).is_err());
    }
}
