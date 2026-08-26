//! Ergonomic proc-macro scaffolding for tpt-teleop (spec §13).
//!
//! # `#[derive(Robot)]`
//!
//! Assemble a robot from a plain struct whose fields are tagged with device
//! attributes. The derive performs three codegen passes (the three Phase-13
//! "Macro codegen" items):
//!
//! 1. **Lock-free rings from struct fields** — for every `#[camera(..)]` /
//!    `#[motor(..)]` field a `tpt_t_ring::SpscRing<Element>` is generated in a
//!    companion `<Robot>Channels` struct returned by `channels()`.
//! 2. **Thread-pinning setup** — `launch(self, &CoreProfile)` moves each
//!    device into a dedicated, CPU-pinned thread (via
//!    `tpt_t_core::affinity::spawn_pinned`) when `thread_per_core = true`, or
//!    a plain thread otherwise, and calls `RobotDevice::run` on it.
//! 3. **Zero-copy serialization boilerplate** — per device, `serialize_*`,
//!    `push_*`, and `pop_*` wrappers wire the element type straight into the
//!    `tpt_t_core::ser` rkyv path with no intermediate allocation.
//!
//! ```ignore
//! use tpt_t::Robot;
//! use tpt_t_core::robot::RobotDevice;
//! use tpt_t_core::profile::CoreProfile;
//! use rkyv::{Archive, Serialize, Deserialize};
//!
//! #[derive(Robot)]
//! #[robot(thread_per_core = true)]
//! struct MyBot {
//!     #[camera(id = 0, element = Frame, capacity = 256)]
//!     cam: Camera,
//!     #[motor(id = 1, element = Command, capacity = 256)]
//!     arm: Motor,
//! }
//! ```
//!
//! Field attribute args:
//! * `id` — logical device id (optional; defaults to field position).
//! * `element` — ring/serialization payload type (optional; defaults to the
//!   field's own type). This is what flows through the SPSC ring.
//! * `capacity` — ring capacity in elements (optional; defaults to 1024).
//! * `role` — overrides the core-profile [`Role`](tpt_t_core::profile::Role)
//!   the device pins to (optional; `camera` ⇒ `Video`, `motor` ⇒ `Control`).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident, LitInt, LitStr, Type, parse_macro_input};

/// Derive entry point. Declares the helper attributes `robot`, `camera`,
/// `motor` so the compiler accepts them on the struct and its fields.
#[proc_macro_derive(Robot, attributes(robot, camera, motor))]
pub fn derive_robot(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// What kind of device a tagged field is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    Camera,
    Motor,
}

/// Per-field parsed configuration.
struct FieldCfg {
    ident: Ident,
    id: usize,
    element: Type,
    capacity: usize,
    role: RoleLit,
}

/// The resolved `Role` variant path generated into user code.
#[derive(Clone, Copy)]
enum RoleLit {
    Video,
    Control,
    Network,
    Input,
    Media,
    Storage,
    Spare,
}

impl RoleLit {
    fn path(self) -> TokenStream2 {
        match self {
            RoleLit::Video => quote!(tpt_t_core::profile::Role::Video),
            RoleLit::Control => quote!(tpt_t_core::profile::Role::Control),
            RoleLit::Network => quote!(tpt_t_core::profile::Role::Network),
            RoleLit::Input => quote!(tpt_t_core::profile::Role::Input),
            RoleLit::Media => quote!(tpt_t_core::profile::Role::Media),
            RoleLit::Storage => quote!(tpt_t_core::profile::Role::Storage),
            RoleLit::Spare => quote!(tpt_t_core::profile::Role::Spare),
        }
    }
}

impl RoleLit {
    fn parse_str(s: &str) -> syn::Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "video" => RoleLit::Video,
            "control" => RoleLit::Control,
            "network" => RoleLit::Network,
            "input" => RoleLit::Input,
            "media" => RoleLit::Media,
            "storage" => RoleLit::Storage,
            "spare" => RoleLit::Spare,
            other => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "unknown robot role {other:?} (expected video/control/network/input/media/storage/spare)"
                    ),
                ));
            }
        })
    }
}

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => &n.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(Robot)] requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(Robot)] can only be applied to a struct",
            ));
        }
    };

    // --- Container-level #[robot(..)] -----------------------------------------
    let mut thread_per_core = true; // default per spec §13
    for attr in &input.attrs {
        if !attr.path().is_ident("robot") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("thread_per_core") {
                let v: syn::LitBool = meta.value()?.parse()?;
                thread_per_core = v.value;
                Ok(())
            } else {
                Err(syn::Error::new_spanned(
                    &meta.path,
                    "unknown #[robot] key (only `thread_per_core` is supported)",
                ))
            }
        })?;
    }

    // --- Per-field #[camera(..)] / #[motor(..)] -------------------------------
    let mut cfgs: Vec<FieldCfg> = Vec::new();
    for (idx, f) in fields.iter().enumerate() {
        for attr in &f.attrs {
            let kind = if attr.path().is_ident("camera") {
                DeviceKind::Camera
            } else if attr.path().is_ident("motor") {
                DeviceKind::Motor
            } else {
                continue;
            };
            let mut id: Option<usize> = None;
            let mut element: Option<Type> = None;
            let mut capacity: usize = 1024;
            let mut role: Option<RoleLit> = None;
            let field_ident = f.ident.clone().unwrap();
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    let v: LitInt = meta.value()?.parse()?;
                    id = Some(v.base10_parse()?);
                } else if meta.path.is_ident("element") {
                    element = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("capacity") {
                    let v: LitInt = meta.value()?.parse()?;
                    capacity = v.base10_parse()?;
                } else if meta.path.is_ident("role") {
                    let v: LitStr = meta.value()?.parse()?;
                    role = Some(RoleLit::parse_str(&v.value())?);
                } else {
                    return Err(syn::Error::new_spanned(
                        &meta.path,
                        "unknown key (expected id/element/capacity/role)",
                    ));
                }
                Ok(())
            })?;
            let role = role.unwrap_or(match kind {
                DeviceKind::Camera => RoleLit::Video,
                DeviceKind::Motor => RoleLit::Control,
            });
            cfgs.push(FieldCfg {
                ident: field_ident,
                id: id.unwrap_or(idx),
                element: element.unwrap_or_else(|| f.ty.clone()),
                capacity,
                role,
            });
        }
    }

    if cfgs.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "#[derive(Robot)] found no #[camera] or #[motor] fields to wire up",
        ));
    }

    let channels_name = format_ident!("{}Channels", name);

    // --- 1. Lock-free rings from struct fields -------------------------------
    let ring_fields = cfgs.iter().map(|c| {
        let ident = &c.ident;
        let elem = &c.element;
        quote! { pub #ident: tpt_t_ring::SpscRing<#elem> }
    });
    let ring_inits = cfgs.iter().map(|c| {
        let ident = &c.ident;
        let cap = c.capacity;
        quote! { #ident: tpt_t_ring::SpscRing::with_capacity(#cap) }
    });

    // --- 2. Thread-pinning launch --------------------------------------------
    let launch_dests = cfgs.iter().map(|c| &c.ident);
    let launch_bodies = cfgs.iter().map(|c| {
        let ident = &c.ident;
        let role = c.role.path();
        let fname = ident.to_string();
        quote! {
            {
                let role = #role;
                let cores = profile.cores_for(role);
                let f = move || { tpt_t_core::robot::RobotDevice::run(#ident); };
                let handle = if <Self>::THREAD_PER_CORE {
                    tpt_t_core::affinity::spawn_pinned(#fname, cores, f)?
                } else {
                    ::std::thread::Builder::new().name(#fname.to_string()).spawn(f)?
                };
                handles.push(handle);
            }
        }
    });

    // --- 3. Zero-copy serialization boilerplate ------------------------------
    let ser_fns = cfgs.iter().map(|c| {
        let ident = &c.ident;
        let elem = &c.element;
        let ser = format_ident!("serialize_{}", ident);
        let push = format_ident!("push_{}", ident);
        let pop = format_ident!("pop_{}", ident);
        quote! {
            /// Serializes one element directly into a pre-allocated wire buffer
            /// (zero-copy rkyv path; no intermediate heap allocation).
            pub fn #ser(value: &#elem, buf: &mut tpt_t_core::ser::AlignedBuf)
                -> ::core::result::Result<usize, tpt_t_core::ser::WireError>
            {
                tpt_t_core::ser::serialize_into(value, buf)
            }
            /// Pushes one element into this device's lock-free SPSC ring.
            pub fn #push(ch: &#channels_name, value: #elem)
                -> ::core::result::Result<(), #elem>
            {
                ch.#ident.push(value)
            }
            /// Pops one element from this device's lock-free SPSC ring.
            pub fn #pop(ch: &#channels_name) -> ::core::option::Option<#elem> {
                ch.#ident.pop()
            }
        }
    });

    let role_list = cfgs.iter().map(|c| c.role.path());

    // Per-device logical id constants (from the `id =` attribute).
    let id_consts = cfgs.iter().map(|c| {
        let ident = format_ident!("{}_ID", c.ident.to_string().to_uppercase());
        let id = c.id;
        quote! { pub const #ident: usize = #id; }
    });

    let expanded = quote! {
        /// Companion channel bundle: one lock-free SPSC ring per device field.
        pub struct #channels_name {
            #(#ring_fields),*
        }

        impl #impl_generics #name #ty_generics #where_clause {
            /// True when `launch` pins each device to its own dedicated core.
            pub const THREAD_PER_CORE: bool = #thread_per_core;

            /// Builds the lock-free channel bundle for every wired device.
            pub fn channels(&self) -> #channels_name {
                #channels_name {
                    #(#ring_inits),*
                }
            }

            /// Roles this robot pins, in field declaration order.
            pub fn roles() -> &'static [tpt_t_core::profile::Role] {
                &[ #(#role_list),* ]
            }

            /// Logical device ids, in field declaration order.
            #(#id_consts)*

            /// Moves each wired device into a dedicated thread (pinned to its
            /// core-profile role when `thread_per_core` is set) and runs it.
            ///
            /// Consumes `self`: each device is moved into its own thread via
            /// [`RobotDevice::run`](tpt_t_core::robot::RobotDevice::run).
            pub fn launch(self, profile: &tpt_t_core::profile::CoreProfile)
                -> ::std::io::Result<::std::vec::Vec<::std::thread::JoinHandle<()>>>
            {
                let #name { #(#launch_dests,)* .. } = self;
                let mut handles: ::std::vec::Vec<::std::thread::JoinHandle<()>> =
                    ::std::vec::Vec::new();
                #(#launch_bodies)*
                ::std::result::Result::Ok(handles)
            }

            #(#ser_fns)*
        }
    };

    Ok(expanded)
}
