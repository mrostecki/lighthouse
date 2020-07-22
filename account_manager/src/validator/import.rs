use crate::{common::ensure_dir_exists, VALIDATOR_DIR_FLAG};
use account_utils::{
    eth2_keystore::Keystore,
    validator_definitions::{
        recursively_find_voting_keystores, ValidatorDefinition, ValidatorDefinitions,
        CONFIG_FILENAME,
    },
    ZeroizeString,
};
use clap::{App, Arg, ArgMatches};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

pub const CMD: &str = "import";
pub const KEYSTORE_FLAG: &str = "keystore";
pub const DIR_FLAG: &str = "directory";
pub const NO_TTY_FLAG: &str = "no-tty";

pub const PASSWORD_PROMPT: &str = "Enter a password, or press enter to omit a password:";

pub fn cli_app<'a, 'b>() -> App<'a, 'b> {
    App::new(CMD)
        .about(
            "Reads existing EIP-2335 keystores and imports them into a Lighthouse \
            validator client.",
        )
        .arg(
            Arg::with_name(KEYSTORE_FLAG)
                .long(KEYSTORE_FLAG)
                .value_name("KEYSTORE_PATH")
                .help("Path to a single keystore to be imported.")
                .conflicts_with(DIR_FLAG)
                .required_unless(DIR_FLAG)
                .takes_value(true),
        )
        .arg(
            Arg::with_name(DIR_FLAG)
                .long(DIR_FLAG)
                .value_name("KEYSTORES_DIRECTORY")
                .help(
                    "Path to a directory which contains zero or more keystores \
                    for import. This directory and all sub-directories will be \
                    searched and any file name which contains 'keystore' and \
                    has the '.json' extension will be attempted to be imported.",
                )
                .conflicts_with(KEYSTORE_FLAG)
                .required_unless(KEYSTORE_FLAG)
                .takes_value(true),
        )
        .arg(
            Arg::with_name(VALIDATOR_DIR_FLAG)
                .long(VALIDATOR_DIR_FLAG)
                .value_name("VALIDATOR_DIRECTORY")
                .help(
                    "The path where the validator directories will be created. \
                    Defaults to ~/.lighthouse/validators",
                )
                .takes_value(true),
        )
        .arg(
            Arg::with_name(NO_TTY_FLAG)
                .long(NO_TTY_FLAG)
                .help("If present, read passwords from stdin instead of tty."),
        )
}

pub fn cli_run(matches: &ArgMatches) -> Result<(), String> {
    let keystore: Option<PathBuf> = clap_utils::parse_optional(matches, KEYSTORE_FLAG)?;
    let keystores_dir: Option<PathBuf> = clap_utils::parse_optional(matches, DIR_FLAG)?;
    let validator_dir = clap_utils::parse_path_with_default_in_home_dir(
        matches,
        VALIDATOR_DIR_FLAG,
        PathBuf::new().join(".lighthouse").join("validators"),
    )?;
    let no_tty = matches.is_present(NO_TTY_FLAG);

    ensure_dir_exists(&validator_dir)?;

    let mut defs = ValidatorDefinitions::open_or_create(&validator_dir)
        .map_err(|e| format!("Unable to open {}: {:?}", CONFIG_FILENAME, e))?;

    // Collect the paths for the keystores that should be imported.
    let keystore_paths = match (keystore, keystores_dir) {
        (Some(keystore), None) => vec![keystore],
        (None, Some(keystores_dir)) => {
            let mut keystores = vec![];

            recursively_find_voting_keystores(&keystores_dir, &mut keystores)
                .map_err(|e| format!("Unable to search {:?}: {:?}", keystores_dir, e))?;

            if keystores.is_empty() {
                eprintln!("No keystores found in {:?}", keystores_dir);
                return Ok(());
            }

            keystores
        }
        _ => {
            return Err(format!(
                "Must supply either --{} or --{}",
                KEYSTORE_FLAG, DIR_FLAG
            ))
        }
    };

    // For each keystore:
    //
    // - Obtain the keystore password, if the user desires.
    // - Move the keystore into the `validator_dir`.
    // - Add the keystore to the validator definitions file.
    //
    // Exit early if any operation fails.
    for keystore_path in &keystore_paths {
        // Fail early if we can't read and write the keystore. This will prevent some more awkward
        // failures later on.
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open(&keystore_path)
            .map_err(|e| {
                format!(
                    "Unable to get read and write permissions on keystore {:?}: {}",
                    keystore_path, e
                )
            })?;

        let keystore = Keystore::from_json_file(keystore_path)
            .map_err(|e| format!("Unable to read keystore JSON {:?}: {:?}", keystore_path, e))?;

        eprintln!("");
        eprintln!("Keystore found at {:?}:", keystore_path);
        eprintln!("");
        eprintln!(" - Public key: 0x{}", keystore.pubkey());
        eprintln!(" - UUID: {}", keystore.uuid());
        eprintln!("");
        eprintln!(
            "If you enter a password it will be stored in {} so that it is not required \
             each time the validator client starts.",
            CONFIG_FILENAME
        );

        let password_opt = loop {
            eprintln!("");
            eprintln!("{}", PASSWORD_PROMPT);

            let password = read_password(no_tty)?;

            if password.as_ref().is_empty() {
                eprintln!("Continuing without password.");
                sleep(Duration::from_secs(1)); // Provides nicer UX.
                break None;
            }

            match keystore.decrypt_keypair(password.as_ref()) {
                Ok(_) => {
                    eprintln!("Password is correct.");
                    eprintln!("");
                    sleep(Duration::from_secs(1)); // Provides nicer UX.
                    break Some(password);
                }
                Err(eth2_keystore::Error::InvalidPassword) => {
                    eprintln!("Invalid password");
                }
                Err(e) => return Err(format!("Error whilst decrypting keypair: {:?}", e)),
            }
        };

        // The keystore is placed in a directory that matches the name of the public key. This
        // provides some loose protection against adding the same keystore twice.
        let dest_dir = validator_dir.join(format!("0x{}", keystore.pubkey()));
        if dest_dir.exists() {
            return Err(format!(
                "Refusing to re-import an existing public key: {:?}",
                keystore_path
            ));
        }

        fs::create_dir_all(&dest_dir)
            .map_err(|e| format!("Unable to create import directory: {:?}", e))?;

        // Retain the keystore file name, but place it in the new directory.
        let moved_path = keystore_path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .map(|file_name_str| dest_dir.join(file_name_str))
            .ok_or_else(|| format!("Badly formatted file name: {:?}", keystore_path))?;

        // Copy the keystore to the new location.
        fs::copy(&keystore_path, &moved_path)
            .map_err(|e| format!("Unable to copy keystore: {:?}", e))?;

        // Attempt to make the move atomic in the case where the copy succeeds but the remove
        // fails.
        if let Err(e) = fs::remove_file(&keystore_path) {
            if keystore_path.exists() {
                // If the original keystore path still exists we can delete the copied one.
                //
                // It is desirable to avoid duplicate keystores since this is how slashing
                // conditions can happen.
                fs::remove_file(moved_path)
                    .map_err(|e| format!("Unable to remove copied keystore: {:?}", e))?;
                return Err(format!("Unable to delete {:?}: {:?}", keystore_path, e));
            } else {
                return Err(format!("An error occurred whilst moving files: {:?}", e));
            }
        }

        eprintln!("Successfully moved keystore.");

        let validator_def =
            ValidatorDefinition::new_keystore_with_password(&moved_path, password_opt)
                .map_err(|e| format!("Unable to create new validator definition: {:?}", e))?;

        defs.push(validator_def);

        defs.save(&validator_dir)
            .map_err(|e| format!("Unable to save {}: {:?}", CONFIG_FILENAME, e))?;

        eprintln!("Successfully updated {}.", CONFIG_FILENAME);
    }

    eprintln!("");
    eprintln!("Successfully imported {} validators.", keystore_paths.len());

    Ok(())
}

/// Reads a password from either TTY or stdin, depeding on the `no_tty` parameter.
fn read_password(no_tty: bool) -> Result<ZeroizeString, String> {
    let result = if no_tty {
        rpassword::prompt_password_stderr("")
            .map_err(|e| format!("Error reading from stdin: {}", e))
    } else {
        rpassword::read_password_from_tty(None)
            .map_err(|e| format!("Error reading from tty: {}", e))
    };

    result.map(ZeroizeString::from)
}
