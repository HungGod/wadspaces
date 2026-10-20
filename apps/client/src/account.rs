//! The person's account from the app (crates/wad-account does the work, on
//! tokio): signing in with GitHub through the sign-in window (the
//! client-signin helper), the username, linking this machine, and the
//! machine's GitHub token. The web app's data/cloud/auth.ts,
//! components/machine/GithubSignIn.tsx and MachineGate's linking.

use std::cell::RefCell;
use std::sync::Arc;

use gtk::{gio, glib};
use wad_account::{Account, Config, signin};
use wad_proto::github::GithubStatus;

use crate::rt;
use crate::store::{ProfileState, Topic, store};

thread_local! {
    static ACCOUNT: RefCell<Option<Arc<Account>>> = const { RefCell::new(None) };
    /// The last sign-in's GitHub token and whose it is, until this machine
    /// is theirs (then it goes to wadd and the account).
    static PENDING: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// The sign-in window, while it's open.
    static WINDOW: RefCell<Option<gio::Subprocess>> = const { RefCell::new(None) };
}

fn account() -> Option<Arc<Account>> {
    ACCOUNT.with(|a| a.borrow().clone())
}

fn wadd() -> wad_wadd_client::Wadd {
    wad_wadd_client::Wadd::here()
}

/// At start: the account's settings, and whoever was signed in this boot.
pub fn start() {
    let made = Config::load()
        .map_err(|e| e.to_string())
        .and_then(|c| Account::new(c, Config::session_file()).map_err(|e| e.to_string()));
    let acc = match made {
        Ok(a) => Arc::new(a),
        Err(e) => {
            tracing::warn!("account: {e}");
            store().update(|s| {
                s.account.checked = true;
                s.account.unconfigured = Some(e);
                Topic::ACCOUNT
            });
            return;
        }
    };
    ACCOUNT.with(|a| *a.borrow_mut() = Some(acc.clone()));
    glib::spawn_future_local(async move {
        let a = acc.clone();
        let user = rt::run(async move { a.restore().await }).await;
        let later = user.as_ref().is_some_and(|u| github_later(&u.uid));
        store().update(|s| {
            s.account.checked = true;
            s.account.user = user.clone();
            s.account.github_later = later;
            Topic::ACCOUNT
        });
        if user.is_some() {
            refresh_profile().await;
        }
    });
}

/// Looks up the profile again (it's Missing until there's a username), and
/// with one, the library.
pub async fn refresh_profile() {
    let Some(a) = account() else { return };
    let got = rt::run(async move { a.ensure_profile().await }).await;
    let have = matches!(got, Ok(Some(_)));
    store().update(|s| {
        s.account.profile = match got {
            Ok(Some(p)) => ProfileState::Have(p),
            Ok(None) => ProfileState::Missing,
            // Offline or refused: leave it to the next try.
            Err(e) => {
                tracing::warn!("profile: {e}");
                return Topic::NONE;
            }
        };
        Topic::ACCOUNT
    });
    if have {
        refresh_library().await;
    }
}

/// The library: what this machine last read at once, then the account's.
pub async fn refresh_library() {
    let Some(a) = account() else { return };
    if store().read_now(|s| s.library.is_none())
        && let Some(cached) = a.cached_library()
    {
        store().update(|s| {
            s.library = Some(cached);
            Topic::LIBRARY
        });
    }
    let got = rt::run(async move { a.library().await }).await;
    match got {
        Ok(lib) => store().update(|s| {
            if s.library.as_ref() == Some(&lib) {
                return Topic::NONE;
            }
            s.library = Some(lib);
            Topic::LIBRARY
        }),
        Err(e) => tracing::warn!("library: {e}"),
    }
}

/// The machines' heartbeats, read again (the Machines page, every 30 s while
/// it's on screen).
pub async fn refresh_machines() {
    let Some(a) = account().filter(|a| a.user().is_some()) else { return };
    match rt::run(async move { a.machines().await }).await {
        Ok(m) => store().update(|s| match s.library.as_mut() {
            Some(l) if l.machines != m => {
                l.machines = m;
                Topic::LIBRARY
            }
            _ => Topic::NONE,
        }),
        Err(e) => tracing::warn!("machines: {e}"),
    }
}

/// A wadspace saved to the account as it is, and here.
pub async fn save_wadspace(w: serde_json::Value) -> Result<(), String> {
    let a = account().filter(|a| a.user().is_some()).ok_or("Sign in first.")?;
    let doc = w.clone();
    rt::run(async move { a.save_wadspace(&doc).await }).await.map_err(|e| e.to_string())?;
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            put(l.wadspaces_for(&w), w);
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
    Ok(())
}

/// A saved WadBrowser setup saved to the account as it is, and here.
pub async fn save_browser(b: serde_json::Value) -> Result<(), String> {
    let a = account().filter(|a| a.user().is_some()).ok_or("Sign in first.")?;
    let doc = b.clone();
    rt::run(async move { a.save_doc("browsers", &doc).await }).await.map_err(|e| e.to_string())?;
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            put(&mut l.browsers, b);
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
    Ok(())
}

/// A document written as it is (the board's plan has the library as it'll
/// be already): `collection` is "templates", "extensions" or "wadspaces".
pub async fn write_doc(collection: &'static str, doc: serde_json::Value) -> Result<(), String> {
    let a = account().filter(|a| a.user().is_some()).ok_or("Sign in first.")?;
    rt::run(async move {
        if collection == "wadspaces" { a.save_wadspace(&doc).await } else { a.save_doc(collection, &doc).await }
    })
    .await
    .map_err(|e| e.to_string())
}

/// A document deleted for good (`collection`: "templates", "extensions"
/// or "wadspaces"), and from the library here.
pub async fn delete_doc(collection: &'static str, id: String) -> Result<(), String> {
    let a = account().filter(|a| a.user().is_some()).ok_or("Sign in first.")?;
    let id2 = id.clone();
    rt::run(async move {
        if collection == "wadspaces" { a.delete_wadspace(&id2).await } else { a.delete_doc(collection, &id2).await }
    })
    .await
    .map_err(|e| e.to_string())?;
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            let list = match collection {
                "templates" => &mut l.templates,
                "extensions" => {
                    l.customizations.retain(|x| x["id"] != id.as_str());
                    &mut l.extensions
                }
                _ => {
                    l.v4.retain(|x| x["id"] != id.as_str());
                    &mut l.wadspaces
                }
            };
            list.retain(|x| x["id"] != id.as_str());
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
    Ok(())
}

/// They've had the tour: so the profile says (here too).
pub async fn set_onboarded() -> Result<(), String> {
    let a = account().filter(|a| a.user().is_some()).ok_or("Sign in first.")?;
    rt::run(async move { a.set_onboarded().await }).await.map_err(|e| e.to_string())?;
    store().update(|s| match &mut s.account.profile {
        ProfileState::Have(p) => {
            p.onboarded = true;
            Topic::ACCOUNT
        }
        _ => Topic::NONE,
    });
    Ok(())
}

/// A document (`collection`: "templates", "extensions", "wadspaces") as
/// the library here has it now.
pub fn put_in_library(collection: &str, doc: serde_json::Value) {
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            let list = match collection {
                "templates" => &mut l.templates,
                "extensions" => &mut l.extensions,
                _ => l.wadspaces_for(&doc),
            };
            put(list, doc);
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
}

/// A file of the signed-in person's on this machine (None: no one's in).
pub fn data_file(name: &str) -> Option<std::path::PathBuf> {
    account().and_then(|a| a.data_file(name))
}

/// Who's signed in (their uid), for stamping documents.
pub fn uid() -> Option<String> {
    account().and_then(|a| a.user()).map(|u| u.uid)
}

/// `d` in place of the document with its id, else at the end.
fn put(list: &mut Vec<serde_json::Value>, d: serde_json::Value) {
    match list.iter_mut().find(|x| x["id"] == d["id"]) {
        Some(x) => *x = d,
        None => list.push(d),
    }
}

/// Has one of their other machines start, stop or restart a wadspace (its
/// relay runs it within a few seconds).
pub async fn command(machine: String, kind: &'static str, wadspace: String) -> Result<(), String> {
    let a = account().ok_or("This machine has no account settings.")?;
    rt::run(async move { a.send_command(&machine, kind, Some(&wadspace)).await.map(drop) })
        .await
        .map_err(|e| e.to_string())
}

/// Keeps the library current: read again every minute while the app is on
/// screen (changes made elsewhere: the web app, another machine).
pub fn follow_library() {
    glib::timeout_add_seconds_local(60, || {
        let (signed_in, on_screen) = store().read_now(|s| {
            let home = s.view.as_ref().is_none_or(|v| v.view == wad_proto::v1::View::Home);
            (s.profile().is_some(), home)
        });
        if signed_in && on_screen {
            glib::spawn_future_local(refresh_library());
        }
        glib::ControlFlow::Continue
    });
}

/// Where the sign-in window's program is: the image's, else beside this one.
fn helper() -> std::path::PathBuf {
    let installed = std::path::PathBuf::from("/usr/libexec/wadspaces/client-signin");
    if installed.exists() {
        return installed;
    }
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("client-signin"))).unwrap_or(installed)
}

/// GitHub's sign-in in its window: its token and GitHub's id for the person.
async fn github_window(auth_domain: &str) -> Result<(String, String), String> {
    let nonce = signin::nonce().map_err(|e| format!("no randomness: {e}"))?;
    let page = signin::page(auth_domain, &nonce, &signin::scopes())
        .ok_or_else(|| format!("not a Firebase auth domain: {auth_domain}"))?;
    let exe = helper();
    let argv = [exe.as_os_str(), page.as_str().as_ref(), signin::REPLY_HOST.as_ref(), signin::CLOSE_URL.as_ref()];
    let p = gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE)
        .map_err(|e| format!("couldn't open the sign-in window: {e}"))?;
    WINDOW.with(|w| *w.borrow_mut() = Some(p.clone()));
    // Given up after a quarter of an hour.
    let timeout = {
        let p = p.clone();
        glib::timeout_add_seconds_local_once(15 * 60, move || p.force_exit())
    };
    let out = p.communicate_utf8_future(None).await;
    WINDOW.with(|w| w.borrow_mut().take());
    timeout.remove();
    let (stdout, _) = out.map_err(|e| e.to_string())?;
    let line = stdout.map(|s| s.to_string()).unwrap_or_default();
    let reply = serde_json::from_str::<serde_json::Value>(line.trim())
        .ok()
        .and_then(|v| v.get("reply").and_then(|r| r.as_str()).map(String::from))
        .and_then(|r| signin::reply(&r, &nonce));
    match reply {
        Some(signin::Reply::Signed { token, github_id }) => Ok((token, github_id)),
        Some(signin::Reply::Failed(e)) => Err(e),
        None => Err("Sign-in was cancelled.".into()),
    }
}

/// Closes the sign-in window, if it's open.
pub fn cancel_sign_in() {
    if let Some(p) = WINDOW.with(|w| w.borrow_mut().take()) {
        p.force_exit();
    }
}

/// Signs in (or up) with GitHub. The token waits for the machine to be theirs.
pub async fn sign_in() -> Result<(), String> {
    let a = account().ok_or("This machine has no account settings.")?;
    let (token, github_id) = github_window(&a.config().auth_domain).await?;
    let acc = a.clone();
    let t = token.clone();
    let user = rt::run(async move { acc.sign_in_github(&t).await }).await.map_err(|e| e.to_string())?;
    PENDING.with(|p| *p.borrow_mut() = Some((token, github_id)));
    let later = github_later(&user.uid);
    store().update(|s| {
        s.account.user = Some(user);
        s.account.profile = ProfileState::Unknown;
        s.account.github_later = later;
        Topic::ACCOUNT
    });
    refresh_profile().await;
    Ok(())
}

/// Signs in to GitHub again (the machine's token was refused or missing).
pub async fn github_again() -> Result<String, String> {
    let a = account().ok_or("This machine has no account settings.")?;
    let want = a.user().map(|u| u.github_id).unwrap_or_default();
    let (token, github_id) = github_window(&a.config().auth_domain).await?;
    if github_id != want {
        return Err("That's another GitHub account: sign in with the one you use for WadSpaces.".into());
    }
    PENDING.with(|p| *p.borrow_mut() = Some((token, github_id)));
    save_pending().await.map(|(login, _)| login)
}

/// The waiting token, to wadd (git, packages, the repo list) and the
/// account (the person's other machines): once this machine is theirs.
/// The GitHub login, and whether the account has it too.
pub async fn save_pending() -> Result<(String, bool), String> {
    let Some((token, github_id)) = PENDING.with(|p| p.borrow().clone()) else {
        return Err("No GitHub sign-in to save.".into());
    };
    let a = account().ok_or("This machine has no account settings.")?;
    if a.user().map(|u| u.github_id) != Some(github_id) {
        PENDING.with(|p| p.borrow_mut().take());
        return Err("That GitHub sign-in was someone else's.".into());
    }
    let w = wadd();
    let t = token.clone();
    rt::run(async move {
        w.act(wad_wadd_client::Method::PUT, "/v1/secrets/github_token", Some(&serde_json::json!({ "value": t }))).await
    })
    .await
    .map_err(|e| format!("couldn't give wadd the token: {e}"))?;
    let acc = a.clone();
    let saved = rt::run(async move { acc.save_github_token(&token).await }).await;
    if let Err(e) = &saved {
        tracing::warn!("GitHub token not saved to the account: {e}");
    }
    PENDING.with(|p| p.borrow_mut().take());
    let status = check_github().await;
    Ok((status.and_then(|s| s.login).unwrap_or_default(), saved.is_ok()))
}

pub fn has_pending() -> bool {
    PENDING.with(|p| p.borrow().is_some())
}

/// Whether this machine's GitHub token works (a refused one counts as
/// none; one that can't be checked offline counts as fine).
pub async fn check_github() -> Option<GithubStatus> {
    let w = wadd();
    let got = rt::run(async move { w.get::<GithubStatus>("/v1/github").await }).await;
    let ok = match &got {
        Ok(s) => s.token && !refused(s.error.as_deref()),
        Err(_) => false,
    };
    store().update(|s| {
        s.account.github = Some(ok);
        Topic::ACCOUNT
    });
    got.ok()
}

/// wadd's words for a token GitHub refuses.
fn refused(error: Option<&str>) -> bool {
    error.is_some_and(|e| {
        let e = e.to_lowercase();
        ["refused", "revoked", "bad credentials", "401"].iter().any(|w| e.contains(w))
    })
}

/// Picks a username (first sign-in, when the GitHub login can't be one).
pub async fn claim_username(username: String, display: String) -> Result<(), String> {
    let a = account().ok_or("This machine has no account settings.")?;
    let p = rt::run(async move { a.claim_username(&username, &display).await }).await.map_err(|e| e.to_string())?;
    store().update(|s| {
        s.account.profile = ProfileState::Have(p);
        Topic::ACCOUNT
    });
    Ok(())
}

/// Links this machine to the signed-in account: a one-time code from the
/// account, redeemed by wadd (it then heartbeats and syncs as theirs).
pub async fn link(machine_name: String) -> Result<(), String> {
    let a = account().ok_or("This machine has no account settings.")?;
    let code = rt::run(async move { a.create_enroll_code(&machine_name).await }).await.map_err(|e| e.to_string())?;
    let w = wadd();
    rt::run(async move {
        w.act(wad_wadd_client::Method::POST, "/v1/cloud/link", Some(&serde_json::json!({ "code": code }))).await
    })
    .await
    .map_err(|e| e.to_string())
}

pub fn sign_out() {
    cancel_sign_in();
    crate::toast::show("Signed out", None, crate::toast::Tone::Info);
    if let Some(a) = account() {
        a.sign_out();
    }
    PENDING.with(|p| p.borrow_mut().take());
    store().update(|s| {
        s.account.user = None;
        s.account.profile = ProfileState::Unknown;
        s.account.github = None;
        s.library = None;
        Topic::ACCOUNT | Topic::LIBRARY
    });
    // Nothing of theirs stays for whoever signs in next.
    crate::pages::builder::editor::cancel();
    crate::pages::builder::preview::close();
    crate::pages::builder::state::reset();
    crate::tour::reset();
    crate::notices::reset();
}

pub fn suggested_username() -> String {
    account().map(|a| a.suggested_username()).unwrap_or_default()
}

/// "Later" on the GitHub step, per person (~/.config/wadspaces/github-later).
fn later_file() -> std::path::PathBuf {
    wad_gtk::theme::file().with_file_name("github-later")
}

fn github_later(uid: &str) -> bool {
    std::fs::read_to_string(later_file()).is_ok_and(|t| t.lines().any(|l| l == uid))
}

pub fn set_github_later(uid: &str) {
    let mut t = std::fs::read_to_string(later_file()).unwrap_or_default();
    if !t.lines().any(|l| l == uid) {
        t.push_str(uid);
        t.push('\n');
        let _ = std::fs::write(later_file(), t);
    }
    store().update(|s| {
        s.account.github_later = true;
        Topic::ACCOUNT
    });
}
