//! Write extensions for Centrepiece.
//!
//! An extension is a WebAssembly component that owns one prefix of Centrepiece,
//! the way the built-in `bm` or `cb` extensions do. Centrepiece loads it at
//! runtime from `~/.config/centrepiece/extensions/<id>/`, runs it in a sandbox,
//! and lets it reach only what its manifest asks for.
//!
//! Implement [`Extension`], then name the type with [`export_extension!`]:
//!
//! ```ignore
//! use centrepiece_extension::{Item, Extension, Response, Screen,
//! export_extension, host};
//!
//! struct Greeter;
//!
//! impl Extension for Greeter {
//!     fn new() -> Self {
//!         Greeter
//!     }
//!
//!     fn activate(&mut self) -> Response {
//!         // A fixed list: Centrepiece does the filtering and the ranking.
//!         Response::Replace(Screen::search(vec![
//!             Item::new("hello", "Hello").glyph('👋'),
//!             Item::new("bye", "Goodbye").glyph('🫡'),
//!         ]).host_filtered())
//!     }
//!
//!     fn select(&mut self, id: &str) -> Response {
//!         host::copy(id);
//!         Response::Dismiss
//!     }
//! }
//!
//! export_extension!(Greeter);
//! ```
//!
//! Build it with `cargo build --release --target wasm32-wasip2`, and put the
//! `.wasm` next to an `extension.toml` — see this crate's README, and the
//! official extensions at <https://github.com/centrepieceapp/extensions>.
//!
//! # Doing slow things
//!
//! Every call into an extension should return quickly: Centrepiece waits a few
//! milliseconds for the answer before showing what it has. Anything slow —
//! [`http`] requests, keychain reads, [programs](host::exec) — is started
//! through [`host`], which hands back a [`TaskId`] at once and delivers the
//! result to [`Extension::task_finished`] later. [`Tasks`] keeps track of what
//! each id was for.
//!
//! # Storing things
//!
//! The extension's own data folder is mounted at [`DATA_DIR`]; `std::fs` works
//! there, and in the folders `permissions.read` lists, read-only and at the
//! same paths as on the Mac — [`home_dir`] finds `~`. Secrets belong in the
//! keychain instead, through [`host::store_secret`].

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "centrepiece-extension",
        pub_export_macro: true,
        export_macro_name: "__export_centrepiece_extension",
        default_bindings_module: "::centrepiece_extension::bindings",
        additional_derives: [PartialEq, Eq],
    });
}

pub use bindings::centrepiece::extension::host::Level;
pub use bindings::centrepiece::extension::types::{
    App, Candidate, Entered, Filter, Header, HttpRequest, HttpResponse, Icon, Item, Method, Mode,
    Output, Response, Screen, SystemAction, TaskId, TaskResult,
};

/// Where the extension's own data folder is mounted.
pub const DATA_DIR: &str = "/data";

/// The user's home folder, for building paths under the folders
/// `permissions.read` lists. `None` for an extension granted none of them.
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// One extension.
///
/// Mirrors the trait Centrepiece's built-in extensions implement. Only
/// [`Extension::new`] and [`Extension::select`] are required; a fixed list that
/// Centrepiece filters is [`Extension::activate`] plus those two.
pub trait Extension: 'static {
    /// Called once, the first time Centrepiece talks to the extension.
    fn new() -> Self
    where
        Self: Sized;

    /// Centrepiece started, or reloaded the extension. The only hook that runs
    /// before the user has summoned anything; set up [`host::set_shortcuts`]
    /// here.
    fn started(&mut self) {}

    /// Centrepiece was summoned. Called on every summon, entered or not, so
    /// it has to be cheap. Update [`host::set_offers`] here.
    fn summoned(&mut self) {}

    /// The user entered the extension. Return the first screen;
    /// [`Extension::search`] follows straight away with the query.
    fn activate(&mut self) -> Response {
        Response::None
    }

    /// The query changed. Not called while a
    /// [host-filtered](Screen::host_filtered) screen is showing.
    fn search(&mut self, query: &str) -> Response {
        let _ = query;
        Response::None
    }

    /// Something was typed at Centrepiece's root, outside any prefix. Rows
    /// returned here are put first — for an extension that recognises what was
    /// typed, such as a color — and picking one calls [`Extension::select`]
    /// with its id, the same as an offer. Called on every keystroke at the
    /// root, so it has to be cheap.
    fn suggest(&mut self, query: &str) -> Vec<Item> {
        let _ = query;
        Vec::new()
    }

    /// A row was picked, by Enter or by its key.
    fn select(&mut self, id: &str) -> Response;

    /// A [`Screen::prompt`] was submitted.
    fn submit(&mut self, value: &str) -> Response {
        let _ = value;
        Response::None
    }

    /// Work started through [`host`] or [`http`] finished.
    fn task_finished(&mut self, task: TaskId, result: TaskResult) -> Response {
        let _ = (task, result);
        Response::None
    }

    /// The user stepped back one screen and the previous one is showing.
    fn popped(&mut self) {}

    /// Centrepiece closed.
    fn dismissed(&mut self) {}
}

/// Makes `$ty` the extension this component exports.
///
/// Only on WebAssembly: a native build — `cargo test`, say — has nobody to
/// export to, and would fail to link if it tried.
#[macro_export]
macro_rules! export_extension {
    ($ty:ty) => {
        #[cfg(target_family = "wasm")]
        type __CentrepieceExtensionExport = $crate::__private::Export<$ty>;
        #[cfg(target_family = "wasm")]
        $crate::bindings::__export_centrepiece_extension!(__CentrepieceExtensionExport);

        /// Stands in for the export on a native build, so the extension is not
        /// mistaken for dead code there.
        #[cfg(not(target_family = "wasm"))]
        #[doc(hidden)]
        pub fn __centrepiece_extension() -> ::std::boxed::Box<dyn $crate::Extension> {
            ::std::boxed::Box::new(<$ty as $crate::Extension>::new())
        }
    };
}

#[doc(hidden)]
pub mod __private {
    use std::marker::PhantomData;

    use super::*;
    use crate::bindings::exports::centrepiece::extension::extension::Guest;

    thread_local! {
        /// The one extension this component holds, made on first use.
        static EXTENSION: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
    }

    fn with<P: Extension, R>(call: impl FnOnce(&mut P) -> R) -> R {
        EXTENSION.with(|cell| {
            let mut slot = cell.borrow_mut();
            let extension = slot.get_or_insert_with(|| Box::new(P::new()));
            call(
                extension
                    .downcast_mut::<P>()
                    .expect("one extension per component"),
            )
        })
    }

    pub struct Export<P>(PhantomData<P>);

    impl<P: Extension> Guest for Export<P> {
        fn started() {
            with::<P, _>(|extension| extension.started())
        }

        fn summoned() {
            with::<P, _>(|extension| extension.summoned())
        }

        fn activate() -> Response {
            with::<P, _>(|extension| extension.activate())
        }

        fn search(query: String) -> Response {
            with::<P, _>(|extension| extension.search(&query))
        }

        fn suggest(query: String) -> Vec<Item> {
            with::<P, _>(|extension| extension.suggest(&query))
        }

        fn select(id: String) -> Response {
            with::<P, _>(|extension| extension.select(&id))
        }

        fn submit(value: String) -> Response {
            with::<P, _>(|extension| extension.submit(&value))
        }

        fn task_finished(task: TaskId, result: TaskResult) -> Response {
            with::<P, _>(|extension| extension.task_finished(task, result))
        }

        fn popped() {
            with::<P, _>(|extension| extension.popped())
        }

        fn dismissed() {
            with::<P, _>(|extension| extension.dismissed())
        }
    }
}

// --- Building screens -------------------------------------------------------

impl Icon {
    /// One of Centrepiece's own icons, by name: `refresh`, `link`, `settings`…
    pub fn builtin(name: impl Into<String>) -> Self {
        Icon::Builtin(name.into())
    }

    /// An SVG from the extension's `assets/` folder, by file name.
    pub fn asset(file: impl Into<String>) -> Self {
        Icon::Asset(file.into())
    }

    pub fn glyph(glyph: char) -> Self {
        Icon::Glyph(glyph.to_string())
    }
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            detail: None,
            icon: Icon::None,
            tint: None,
            key: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    pub fn glyph(self, glyph: char) -> Self {
        self.icon(Icon::glyph(glyph))
    }

    /// Draws a [builtin](Icon::builtin) or [asset](Icon::asset) icon in
    /// `rgba`, `0xRRGGBBAA`, instead of the theme's colour.
    pub fn tint(mut self, rgba: u32) -> Self {
        self.tint = Some(rgba);
        self
    }

    /// On a [`Screen::menu`], the key that picks this row outright, with `cmd`.
    pub fn key(mut self, key: char) -> Self {
        self.key = Some(key.to_string());
        self
    }
}

impl Screen {
    fn new(mode: Mode, items: Vec<Item>) -> Self {
        Self {
            mode,
            filter: Filter::Extension,
            title: None,
            placeholder: None,
            items,
            status: None,
            loading: false,
        }
    }

    /// A list the user narrows by typing. The extension filters it in
    /// [`Extension::search`] unless it is
    /// [host-filtered](Screen::host_filtered).
    pub fn search(items: Vec<Item>) -> Self {
        Self::new(Mode::Search, items)
    }

    /// A fixed list of choices, each picked with `cmd` and its own key, and
    /// filtered by Centrepiece as the user types.
    pub fn menu(title: impl Into<String>, items: Vec<Item>) -> Self {
        Self::new(Mode::Menu, items).title(title)
    }

    /// A single field. `masked` hides what is typed, for secrets.
    pub fn prompt(title: impl Into<String>, placeholder: impl Into<String>, masked: bool) -> Self {
        Self::new(Mode::Prompt(masked), Vec::new())
            .title(title)
            .placeholder(placeholder)
    }

    /// Lets Centrepiece filter and rank the rows as the user types, instead
    /// of calling [`Extension::search`]. Picks are remembered by row id.
    pub fn host_filtered(mut self) -> Self {
        self.filter = Filter::Host;
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
}

impl Response {
    /// [`Response::Enter`]: open `screen` on top, with `query` typed.
    pub fn enter(screen: Screen, query: impl Into<String>) -> Self {
        Response::Enter(Entered {
            screen,
            query: query.into(),
        })
    }
}

// --- Keeping track of work in flight ----------------------------------------

/// What each task in flight was started for.
///
/// ```ignore
/// enum Pending { Repos, Login }
///
/// self.tasks.insert(http::get(url).send(), Pending::Repos);
/// // ...later, in task_finished:
/// match self.tasks.take(task) { Some(Pending::Repos) => ..., _ => ... }
/// ```
pub struct Tasks<T> {
    pending: HashMap<TaskId, T>,
}

impl<T> Default for Tasks<T> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }
}

impl<T> Tasks<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, task: TaskId, what: T) {
        self.pending.insert(task, what);
    }

    /// What `task` was for, forgetting it. `None` for a task this extension did
    /// not start, or already took.
    pub fn take(&mut self, task: TaskId) -> Option<T> {
        self.pending.remove(&task)
    }

    /// Whether any task in flight matches.
    pub fn any(&self, matches: impl Fn(&T) -> bool) -> bool {
        self.pending.values().any(matches)
    }

    /// Forgets the tasks in flight that match; their results will be ignored.
    pub fn forget(&mut self, matches: impl Fn(&T) -> bool) {
        self.pending.retain(|_, what| !matches(what));
    }

    /// Forgets every task in flight; their results will be ignored.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

// --- Centrepiece's services ------------------------------------------------

/// What Centrepiece does on an extension's behalf.
///
/// Anything the manifest has not been granted fails: functions that return a
/// [`TaskId`] deliver an error to [`Extension::task_finished`], the others log
/// a warning and do nothing.
pub mod host {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::bindings::centrepiece::extension::host as raw;

    pub fn log(level: Level, message: impl AsRef<str>) {
        raw::log(level, message.as_ref());
    }

    pub fn info(message: impl AsRef<str>) {
        log(Level::Info, message);
    }

    pub fn warn(message: impl AsRef<str>) {
        log(Level::Warn, message);
    }

    pub fn debug(message: impl AsRef<str>) {
        log(Level::Debug, message);
    }

    /// The extension's `settings` from `centrepiece.yml`. `None` when there are
    /// none, or when they do not fit `T` — which is logged.
    pub fn settings<T: DeserializeOwned>() -> Option<T> {
        let json = raw::settings();
        match serde_json::from_str::<Option<T>>(&json) {
            Ok(settings) => settings,
            Err(err) => {
                warn(format!("ignoring settings that do not fit: {err}"));
                None
            }
        }
    }

    /// Rows offered at the root, found without typing the prefix. Kept until
    /// replaced; set them in [`Extension::started`].
    pub fn set_shortcuts(items: &[Item]) {
        raw::set_shortcuts(items);
    }

    /// Rows put first at the empty root. Kept until replaced; update them in
    /// [`Extension::summoned`].
    pub fn set_offers(items: &[Item]) {
        raw::set_offers(items);
    }

    /// `values` ordered by how well `fields` match `query`, most-picked first
    /// among equals, without those that do not match. `key` names a value for
    /// [`record_pick`].
    pub fn rank<T>(
        query: &str,
        values: Vec<T>,
        key: impl Fn(&T) -> String,
        fields: impl Fn(&T) -> Vec<String>,
    ) -> Vec<T> {
        let candidates: Vec<Candidate> = values
            .iter()
            .map(|value| Candidate {
                key: key(value),
                fields: fields(value),
            })
            .collect();
        let order = raw::rank(query, &candidates);
        let mut slots: Vec<Option<T>> = values.into_iter().map(Some).collect();
        order
            .into_iter()
            .filter_map(|index| slots.get_mut(index as usize)?.take())
            .collect()
    }

    /// Notes that the user picked `key`, so [`rank`] floats it next time.
    pub fn record_pick(key: &str) {
        raw::record_pick(key);
    }

    pub fn read_secret(key: &str) -> TaskId {
        raw::read_secret(key)
    }

    pub fn store_secret(key: &str, value: &[u8]) -> TaskId {
        raw::store_secret(key, value)
    }

    pub fn forget_secret(key: &str) -> TaskId {
        raw::forget_secret(key)
    }

    pub fn copy(text: &str) {
        raw::copy(text);
    }

    /// Locks, sleeps, or starts the screen saver through the native host.
    /// Requires `permissions.system-actions`; errors include permission denial.
    pub fn perform_system_action(action: SystemAction) -> Result<(), String> {
        raw::perform_system_action(action)
    }

    pub fn open_url(url: &str) {
        raw::open_url(url);
    }

    pub fn open_path(path: &str) {
        raw::open_path(path);
    }

    /// What was on the clipboard when Centrepiece was summoned, if it was
    /// text. Read it in [`Extension::summoned`], to offer something for it.
    pub fn clipboard_text() -> Option<String> {
        raw::clipboard_text()
    }

    /// The application that was in front when Centrepiece was summoned —
    /// the one the user was working in. Read it in [`Extension::summoned`].
    pub fn front_app() -> Option<App> {
        raw::front_app()
    }

    /// Starts `program`, a path `permissions.run` lists, and lets it run on
    /// its own. The error says why it could not be started.
    pub fn run(program: &str, args: &[String]) -> Result<(), String> {
        raw::run(program, args)
    }

    /// Runs `program`, a path `permissions.run` lists, to the end. What it
    /// printed arrives in [`Extension::task_finished`] as
    /// [`TaskResult::Output`].
    pub fn exec(program: &str, args: &[&str]) -> TaskId {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        raw::exec(program, &args)
    }
}

impl Output {
    /// Whether the program exited with 0.
    pub fn is_success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.stdout)
    }
}

/// HTTP requests, made by Centrepiece and limited to the hosts listed under
/// `permissions.network` in the manifest.
///
/// ```ignore
/// let task = http::get("https://api.example.com/items")
///     .header("Accept", "application/json")
///     .send();
/// ```
pub mod http {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::bindings::centrepiece::extension::host as raw;

    pub struct Builder {
        request: HttpRequest,
    }

    pub fn request(method: Method, url: impl Into<String>) -> Builder {
        Builder {
            request: HttpRequest {
                method,
                url: url.into(),
                headers: Vec::new(),
                body: None,
            },
        }
    }

    pub fn get(url: impl Into<String>) -> Builder {
        request(Method::Get, url)
    }

    pub fn post(url: impl Into<String>) -> Builder {
        request(Method::Post, url)
    }

    impl Builder {
        pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
            self.request.headers.push(Header {
                name: name.into(),
                value: value.into(),
            });
            self
        }

        pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
            self.request.body = Some(body.into());
            self
        }

        /// A JSON body, with the content type to match.
        pub fn json<T: serde::Serialize>(self, body: &T) -> Self {
            let bytes = serde_json::to_vec(body).expect("serializing to memory cannot fail");
            self.header("Content-Type", "application/json").body(bytes)
        }

        /// Starts the request. Its [`TaskResult::Http`] arrives in
        /// [`Extension::task_finished`].
        pub fn send(self) -> TaskId {
            raw::fetch(&self.request)
        }
    }

    impl HttpResponse {
        pub fn is_success(&self) -> bool {
            (200..300).contains(&self.status)
        }

        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case(name))
                .map(|header| header.value.as_str())
        }

        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }

        pub fn json<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
            serde_json::from_slice(&self.body)
        }
    }
}
