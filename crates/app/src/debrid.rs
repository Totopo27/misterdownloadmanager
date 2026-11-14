//! Debrid service accounts in the app (#44): where the services answer, the account handle the
//! downloads use (`App::debrid`), and Options → Accounts' sign-in (`SignIn`), which runs on a
//! thread of its own while the dialog shows its state.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_debrid::{AccountInfo, Credential, Fault, Http, ServiceKind, Services};

/// Where the services answer. Debug builds only: MDM_DEBRID_LAB=http://127.0.0.1:PORT/ points
/// at the lab server's fakes (labserver `debrid`), for tests by hand.
pub fn services() -> Services {
    #[cfg(debug_assertions)]
    if let Some(base) = std::env::var("MDM_DEBRID_LAB")
        .ok()
        .and_then(|b| url::Url::parse(&b).ok())
    {
        return Services::at(&base);
    }
    Services::default()
}

/// A client for the services, through MDM's own network settings (proxy, DNS, adapter).
pub fn http() -> Result<Arc<Http>, String> {
    Http::new(&crate::settings::own_net(), services()).map(Arc::new)
}

/// Did the browser start this download itself (it came with the browser's size or type,
/// `ipc::Command`)? Then the host is already sending the file: by default it is downloaded
/// directly, not through a debrid account (`Item::direct`).
pub fn browser_started(context: &mdm_ipc::Command) -> bool {
    context.size.is_some() || context.mime.is_some()
}

/// Which account a download goes through (#44): the accounts' handle (`App::debrid`), Options'
/// "Download supported file-host links through these accounts", and each account's name for the
/// windows that say it. Cheap to clone: Download File Info and File Properties keep one.
#[derive(Clone, Default)]
pub struct Route {
    debrid: mdm_debrid::Debrid,
    on: bool,
    /// Each account's id and its name at the service.
    names: Arc<Vec<(u64, String)>>,
}

impl Route {
    pub fn new(debrid: &mdm_debrid::Debrid, settings: &crate::settings::Settings) -> Self {
        Route {
            debrid: debrid.clone(),
            on: settings.debrid_on,
            names: Arc::new(
                settings
                    .debrid_accounts
                    .iter()
                    .map(|a| (a.id, a.user.clone()))
                    .collect(),
            ),
        }
    }

    /// The account whose service takes `url`'s host: None when turned off in Options, or no
    /// account takes it (or its host lists aren't read yet).
    pub fn account(&self, url: &url::Url) -> Option<(u64, ServiceKind)> {
        if !self.on {
            return None;
        }
        self.debrid.account_for(url)
    }

    /// The unlocker a download of `url` goes with (`mdm_engine::Options::unlocker`): none when
    /// it is downloaded directly (`direct`: the user's choice, or the browser had started it).
    pub fn unlocker(&self, url: &url::Url, direct: bool) -> Option<Arc<dyn mdm_engine::Unlocker>> {
        if direct || self.account(url).is_none() {
            return None;
        }
        self.debrid.unlocker_for(url)
    }

    /// The service that takes `url`, and the account's name: "Real-Debrid (labuser)".
    pub fn through(&self, url: &url::Url) -> Option<String> {
        let (id, service) = self.account(url)?;
        let user = self
            .names
            .iter()
            .find(|(a, _)| *a == id)
            .map(|(_, u)| u.as_str())
            .filter(|u| !u.is_empty());
        Some(match user {
            Some(u) => format!("{} ({})", service.name(), crate::i18n::ltr(u)),
            None => service.name().to_owned(),
        })
    }

    /// Just the service's name ("Real-Debrid"), for the narrow places.
    pub fn service(&self, url: &url::Url) -> Option<&'static str> {
        self.account(url).map(|(_, s)| s.name())
    }
}

/// "premium, 143 days left", "free account": what Options shows for an account.
pub fn state_text(info: &AccountInfo) -> String {
    match (info.premium, info.premium_left) {
        (Some(true), Some(left)) => crate::i18n::fmt(
            // i18n: Options → Accounts: a debrid account's state; {days} the days of premium left
            crate::i18n::t("premium, {days} days left"),
            &[("days", &(left.as_secs() / 86400).to_string())],
        ),
        // i18n: Options → Accounts: a debrid account with premium (no end date given)
        (Some(true), None) => crate::i18n::t("premium").to_owned(),
        // i18n: Options → Accounts: a debrid account without premium (it can't unlock links)
        (Some(false), _) => crate::i18n::t("free account: links need premium").to_owned(),
        // i18n: Options → Accounts: the service doesn't say whether the account is premium (Mega-Debrid, signed in with an API key); a free account's links are refused when downloading
        (None, _) => crate::i18n::t("the service doesn't say whether it is premium").to_owned(),
    }
}

/// Why a sign-in or a check failed, in the interface language.
pub fn fault_text(service: ServiceKind, f: &Fault) -> String {
    match f {
        Fault::Service(f) => crate::errors::service(service.name(), f),
        // i18n: Options → Accounts: the service doesn't take this file host (shouldn't happen when signing in)
        Fault::Unsupported => crate::i18n::t("not supported by this service").to_owned(),
    }
}

/// Signing in, as the dialog shows it.
#[derive(Clone, Debug, Default)]
pub enum SignInState {
    #[default]
    Idle,
    /// Asking the service for a code.
    Starting,
    /// The code to type at `page`; the poll runs until `until`.
    Waiting {
        code: String,
        page: String,
        until: Instant,
    },
    /// Checking the account.
    Checking,
    /// Signed in: the account's name and state, and the credential to keep (sealed).
    Done {
        user: String,
        state: String,
        credential: String,
    },
    Failed(String),
}

/// A sign-in running on its own thread; the dialog reads `state` each frame.
#[derive(Clone, Default)]
pub struct SignIn {
    pub state: Arc<Mutex<SignInState>>,
    /// Set when the dialog closes: the poll stops.
    pub stop: Arc<std::sync::atomic::AtomicBool>,
}

impl SignIn {
    pub fn get(&self) -> SignInState {
        self.state.lock().unwrap().clone()
    }

    fn set(&self, s: SignInState, ctx: &egui::Context) {
        *self.state.lock().unwrap() = s;
        ctx.request_repaint();
    }

    /// Sign in by code (Real-Debrid's device flow, AllDebrid's PIN), in the browser.
    pub fn by_code(&self, service: ServiceKind, ctx: &egui::Context) {
        let (me, ctx) = (self.clone(), ctx.clone());
        me.set(SignInState::Starting, &ctx);
        std::thread::spawn(move || {
            let run = async {
                let http = http()?;
                let code = mdm_debrid::signin::start(&http, service)
                    .await
                    .map_err(|f| fault_text(service, &f))?;
                let until = Instant::now() + Duration::from_secs(code.expires_in);
                me.set(
                    SignInState::Waiting {
                        code: code.shown.clone(),
                        page: code.page.clone(),
                        until,
                    },
                    &ctx,
                );
                // The page opens in the browser, except in off-screen test runs and against the
                // lab's fake service (whose page is the real one's address).
                if !crate::offscreen() && std::env::var_os("MDM_DEBRID_LAB").is_none() {
                    crate::launch::open(&code.page);
                }
                loop {
                    if me.stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(String::new());
                    }
                    if Instant::now() >= until {
                        // i18n: Options → Accounts: the sign-in code ran out before it was used
                        return Err(crate::i18n::t("the code expired: sign in again").to_owned());
                    }
                    tokio::time::sleep(Duration::from_secs(code.interval.max(1))).await;
                    if let Some(c) = mdm_debrid::signin::poll(&http, &code)
                        .await
                        .map_err(|f| fault_text(service, &f))?
                    {
                        return Ok((http, c));
                    }
                }
            };
            me.finish(service, run, &ctx);
        });
    }

    /// Sign in with a token the user pasted (the service's API token page).
    pub fn by_token(&self, service: ServiceKind, token: String, ctx: &egui::Context) {
        let (me, ctx) = (self.clone(), ctx.clone());
        me.set(SignInState::Checking, &ctx);
        std::thread::spawn(move || {
            let run = async {
                Ok((
                    http()?,
                    Credential::Token {
                        token: token.trim().to_owned(),
                    },
                ))
            };
            me.finish(service, run, &ctx);
        });
    }

    /// Sign in with the user name and password (Mega-Debrid's other way to its API key).
    pub fn by_password(
        &self,
        service: ServiceKind,
        user: String,
        password: String,
        ctx: &egui::Context,
    ) {
        let (me, ctx) = (self.clone(), ctx.clone());
        me.set(SignInState::Checking, &ctx);
        std::thread::spawn(move || {
            let run = async { Ok((http()?, Credential::Login { user, password })) };
            me.finish(service, run, &ctx);
        });
    }

    /// Run `run` (a credential), check the account it signs in to, and say how it went.
    fn finish(
        &self,
        service: ServiceKind,
        run: impl std::future::Future<Output = Result<(Arc<Http>, Credential), String>>,
        ctx: &egui::Context,
    ) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return self.set(SignInState::Failed(e.to_string()), ctx),
        };
        let out = rt.block_on(async {
            let (http, credential) = run.await?;
            self.set(SignInState::Checking, ctx);
            let info = mdm_debrid::check(&http, service, credential.clone())
                .await
                .map_err(|f| fault_text(service, &f))?;
            let credential = serde_json::to_string(&credential).map_err(|e| e.to_string())?;
            Ok::<_, String>(SignInState::Done {
                user: info.user.clone(),
                state: state_text(&info),
                credential,
            })
        });
        match out {
            Ok(done) => self.set(done, ctx),
            // Closed meanwhile: nothing to say.
            Err(why) if why.is_empty() => self.set(SignInState::Idle, ctx),
            Err(why) => self.set(SignInState::Failed(why), ctx),
        }
    }
}

/// The command line's way to the accounts in Options → Accounts (#44, `mdm_cli::Accounts`): read
/// from the app's settings, which open their sealed secrets, when a download asks, never before.
/// The services are asked through the download's own network options.
pub struct CliAccounts;

impl mdm_cli::Accounts for CliAccounts {
    fn unlocker<'a>(
        &'a self,
        url: &'a url::Url,
        net: &'a mdm_engine::Options,
    ) -> mdm_cli::Unlocking<'a> {
        Box::pin(async move {
            let Some(settings) = crate::store::settings_only() else {
                return Ok(None);
            };
            let accounts: Vec<mdm_debrid::Account> = settings
                .debrid_accounts
                .iter()
                .filter(|a| a.enabled)
                .filter_map(crate::settings::DebridAccount::account)
                .collect();
            if !settings.debrid_on || accounts.is_empty() {
                return Ok(None);
            }
            let http = Http::new(net, services()).map(Arc::new)?;
            let debrid = mdm_debrid::Debrid::default();
            debrid.set_accounts(accounts, http);
            let failed = debrid.refresh_hosts().await.err();
            match (debrid.unlocker_for(url), failed) {
                (Some(u), _) => Ok(Some(u)),
                // A service that didn't answer may take this host: the command line says so.
                (None, Some((service, f))) => Err(format!(
                    "{} didn't say which file hosts it takes ({})",
                    service.name(),
                    match f {
                        Fault::Service(f) => f.to_string(),
                        Fault::Unsupported => "unsupported".into(),
                    }
                )),
                (None, None) => Ok(None),
            }
        })
    }
}
