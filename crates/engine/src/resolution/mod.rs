//! The config-server's resolution model (T10, D82 to D85, D87): how a service's request turns into the content it gets.
//!
//! A request is `(application, profile = "<tenant>,default", [channel], file)`. Two kinds of file are resolved differently,
//! and a file's merge behaviour follows from its role, not from a pattern an admin wrote (D82 supersedes D14):
//!
//! - **Property sources** (`application.*`, `application-*.*`, `<app>.*`, `<app>-<tenant>.*`, in `.properties`, `.yml` or
//!   `.yaml`) merge setting by setting into the **property view** of the application and tenant ([`property_view`]).
//! - **Resource files** (every other file) are chosen whole ([`resolve_resource`]): the tenant's copy, else the base copy,
//!   the channel folder's copy first when the request names a channel. Then every `${KEY}` whose key is in the property view
//!   is replaced ([`render`]); any other `${...}` stays as it is, for the service to fill from its own environment.
//!
//! The folders searched, and their order, are [`search_locations`]; they are fixed when the config-server starts, which is
//! why a new folder or a change to `channels.yml` needs a restart (check C11, [`pickup_state`]).
//!
//! # What these functions are not
//!
//! They read no file and no environment: the caller parses the property sources once (a blob is parsed once, however many
//! swimlanes hold it) and passes the settings in, and a placeholder is never filled from an environment variable's value
//! (the engine has no way to read one; C10 takes environment variable *names*). Nothing here is a clock: `pickup_state`
//! takes the times it compares.
//!
//! # Assumptions the plan records (A6, A10)
//!
//! - A property view key is Spring's: dotted, `[n]` for list items, no `#<doc>/` prefix. The documents of one file merge in
//!   order, the later wins. A list defined by a higher source replaces the lower source's list as a unit (the binder's rule).
//!   Profile activation inside a file (`spring.config.activate.on-profile`) is ignored, as is a profile named `default`.
//! - Only `${KEY}` with `KEY` in `[A-Za-z0-9_.\-\[\]]+` is replaced, once, with no re-expansion of what was inserted.
//!   `${KEY:default}`, nested forms and `$${` are left as written and C10 does not flag them.
//! - A request with a channel prefers the channel folder's tenant copy, then its base copy, then the folder above's.

mod locations;
mod pickup;
mod property_view;
mod render;
mod resource;
mod role;

pub use self::locations::{SearchLocations, is_channel_dir, search_locations};
pub use self::pickup::{
    ClientSettings, DEFAULT_CACHE_TTL_SECS, Pickup, PickupChange, PickupInput, pickup_state,
};
pub use self::property_view::{PropertyFile, PropertyView, ViewSetting, property_view};
pub use self::render::{Placeholder, Rendered, RenderedFile, render, render_file, scan_placeholders};
pub use self::resource::{ResourceChoice, resolve_resource};
pub use self::role::{file_role, logical_file_role, merge_mode};
