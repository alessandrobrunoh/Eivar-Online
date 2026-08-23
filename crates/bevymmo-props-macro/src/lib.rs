//! Procedural macro for defining game props with minimal boilerplate.
//!
//! # Example
//!
//! ```rust,ignore
//! use bevymmo_shared::placeables::props;
//!
//! // Tree: cylindrical trunk collider, blocks movement.
//! #[props(
//!     id = "tree_oak_large",
//!     name = "Oak Tree (Large)",
//!     icon = "🌳",
//!     asset = "models/new/tree_oak_large.glb",
//!     scale = (1.0, 1.0, 1.0),
//!     tint = (0.2, 0.5, 0.2),
//!     blocks_movement = true,
//!     collision = cylinder(radius = 0.4, height = 6.0)
//! )]
//! pub struct TreeOakLargeProp;
//!
//! // Decorative prop: no collision, never blocks.
//! #[props(
//!     id = "pebbles",
//!     name = "Pebbles",
//!     icon = "🪨",
//!     asset = "models/new/pebbles.glb",
//!     blocks_movement = false
//! )]
//! pub struct PebblesProp;
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashMap;
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{bracketed, parenthesized, parse_macro_input};
use syn::{DeriveInput, Ident, LitBool, LitFloat, LitInt, LitStr, Path, Token};

/// Attribute macro to define a placeable prop with minimal boilerplate.
///
/// Automatically implements:
/// - `PlaceableDefinition`
/// - `PropPlaceable`
/// - `register()` function
#[proc_macro_attribute]
pub fn props(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);

    // Parse attribute arguments manually (syn 2.x compatible)
    let attr_str = attr.to_string();
    let attrs = parse_attrs(&attr_str);

    // Extract struct name
    let name = &input.ident;

    // Get values with defaults
    let id = attrs.id.unwrap_or_else(|| name.to_string().to_lowercase());
    let display_name = attrs.display_name.unwrap_or_else(|| id.clone());
    let icon = attrs.icon.unwrap_or_else(|| "❓".to_string());
    let asset = attrs.asset.unwrap_or_else(|| format!("models/{}.glb", id));

    // Destructure tuples for quote! (tuples don't implement ToTokens)
    let (scale_x, scale_y, scale_z) = attrs.scale.unwrap_or((1.0_f32, 1.0_f32, 1.0_f32));
    let (tint_r, tint_g, tint_b) = attrs.tint.unwrap_or((0.5_f32, 0.5_f32, 0.5_f32));
    let blocks_movement = attrs.blocks_movement;

    // Translate the parsed collision DSL into a `CollisionShape` constructor
    // token stream. We intentionally keep the macro self-contained and do not
    // depend on the `CollisionShape` path being imported at the call site:
    // we emit a fully-qualified path so prop authors only need `use ...props`.
    let collision_tokens = build_collision_tokens(attrs.collision);

    // Generate implementation
    let expanded = quote! {
        /// Auto-generated prop definition via #[props] macro.
        #input

        impl crate::placeables::PlaceableDefinition for #name {
            fn id(&self) -> crate::placeables::KindId {
                crate::placeables::KindId::new(#id)
            }

            fn display_name(&self) -> &'static str {
                #display_name
            }

            fn icon(&self) -> &'static str {
                #icon
            }

            fn asset_hint(&self) -> crate::placeables::AssetHint {
                crate::placeables::AssetHint::Scene(#asset)
            }

            fn defaults(&self) -> crate::placeables::PlaceableDefaults {
                crate::placeables::PlaceableDefaults {
                    transform: crate::world::TransformData {
                        translation: [0.0_f32, 0.0_f32, 0.0_f32],
                        rotation_deg: [0.0_f32, 0.0_f32, 0.0_f32],
                        scale: [#scale_x, #scale_y, #scale_z],
                    },
                    tint: Some([#tint_r, #tint_g, #tint_b]),
                    collision: #collision_tokens,
                    blocks_movement: #blocks_movement,
                }
            }
        }

        impl crate::placeables::PropPlaceable for #name {}

        /// Register this prop in the global registry.
        pub fn register(registry: &mut crate::placeables::PlaceableRegistry) {
            registry.register_prop(std::sync::Arc::new(#name))
        }
    };

    TokenStream::from(expanded)
}

/// Attribute macro to define a harvestable resource node.
///
/// Automatically implements:
/// - `PlaceableDefinition`
/// - `ResourceNodePlaceable`
/// - `register()` function
#[proc_macro_attribute]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let attr_str = attr.to_string();
    let attrs = parse_resource_attrs(&attr_str);
    match expand_resource(&input, attrs) {
        Ok(tokens) => tokens.into(),
        Err(message) => syn::Error::new_spanned(&input.ident, message)
            .to_compile_error()
            .into(),
    }
}

struct ResourceAttributes {
    props: PropsAttributes,
    max_pieces: Option<u32>,
    channel_seconds: Option<f32>,
    min_channel_seconds: f32,
    yield_item: Option<String>,
    yield_amount: u32,
    regen_interval_seconds: Option<f32>,
    regen_amount: Option<u32>,
    interact_range: f32,
    bonus_tools: Vec<String>,
}

fn parse_resource_attrs(input: &str) -> ResourceAttributes {
    let mut attrs = ResourceAttributes {
        props: parse_attrs(input),
        max_pieces: None,
        channel_seconds: None,
        min_channel_seconds: 0.25,
        yield_item: None,
        yield_amount: 1,
        regen_interval_seconds: None,
        regen_amount: None,
        interact_range: 2.5,
        bonus_tools: Vec::new(),
    };

    for pair in split_top_level_commas(input) {
        let pair = pair.trim();
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "max_pieces" => attrs.max_pieces = value.parse().ok(),
            "channel_seconds" => attrs.channel_seconds = value.parse().ok(),
            "min_channel_seconds" => {
                if let Ok(min) = value.parse() {
                    attrs.min_channel_seconds = min;
                }
            }
            "yield_item" => attrs.yield_item = Some(clean_string(value)),
            "yield_amount" => {
                if let Ok(amount) = value.parse() {
                    attrs.yield_amount = amount;
                }
            }
            "regen_interval_seconds" => attrs.regen_interval_seconds = value.parse().ok(),
            "regen_amount" => attrs.regen_amount = value.parse().ok(),
            "interact_range" => {
                if let Ok(range) = value.parse() {
                    attrs.interact_range = range;
                }
            }
            "bonus_tools" => attrs.bonus_tools = parse_ident_list(value),
            _ => {}
        }
    }

    attrs
}

fn expand_resource(input: &DeriveInput, attrs: ResourceAttributes) -> Result<TokenStream2, String> {
    let name = &input.ident;
    let id = attrs
        .props
        .id
        .clone()
        .unwrap_or_else(|| name.to_string().to_lowercase());
    let display_name = attrs
        .props
        .display_name
        .clone()
        .unwrap_or_else(|| id.clone());
    let icon = attrs.props.icon.clone().unwrap_or_else(|| "⛏".to_string());
    let asset = attrs
        .props
        .asset
        .clone()
        .ok_or_else(|| "#[resource(...)] requires `asset = \"...\"`".to_string())?;
    let max_pieces = attrs
        .max_pieces
        .filter(|n| *n >= 1)
        .ok_or_else(|| "#[resource(...)] requires `max_pieces = N` with N >= 1".to_string())?;
    let channel_seconds = attrs
        .channel_seconds
        .filter(|n| *n > 0.0)
        .ok_or_else(|| "#[resource(...)] requires `channel_seconds = N` with N > 0".to_string())?;
    let yield_item = attrs
        .yield_item
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "#[resource(...)] requires `yield_item = \"...\"`".to_string())?;
    let regen_interval_seconds = attrs
        .regen_interval_seconds
        .filter(|n| *n > 0.0)
        .ok_or_else(|| {
            "#[resource(...)] requires `regen_interval_seconds = N` with N > 0".to_string()
        })?;
    let regen_amount = attrs
        .regen_amount
        .filter(|n| *n >= 1)
        .ok_or_else(|| "#[resource(...)] requires `regen_amount = N` with N >= 1".to_string())?;
    if attrs.yield_amount < 1 {
        return Err("#[resource(...)] `yield_amount` must be >= 1".to_string());
    }

    let (scale_x, scale_y, scale_z) = attrs.props.scale.unwrap_or((1.0, 1.0, 1.0));
    let tint = attrs.props.tint.map(|(r, g, b)| quote!(Some([#r, #g, #b])));
    let tint_tokens = tint.unwrap_or_else(|| quote!(None));
    let blocks_movement = attrs.props.blocks_movement;
    let collision_tokens = build_collision_tokens(attrs.props.collision);
    let min_channel_seconds = attrs.min_channel_seconds;
    let yield_amount = attrs.yield_amount;
    let interact_range = attrs.interact_range;
    let bonus_tools: Vec<TokenStream2> = attrs
        .bonus_tools
        .iter()
        .map(|kind| {
            let ident = Ident::new(kind, proc_macro2::Span::call_site());
            quote! { crate::items::GatheringToolKind::#ident }
        })
        .collect();

    Ok(quote! {
        #input

        impl crate::placeables::PlaceableDefinition for #name {
            fn id(&self) -> crate::placeables::KindId {
                crate::placeables::KindId::new(#id)
            }

            fn display_name(&self) -> &'static str {
                #display_name
            }

            fn icon(&self) -> &'static str {
                #icon
            }

            fn asset_hint(&self) -> crate::placeables::AssetHint {
                crate::placeables::AssetHint::Scene(#asset)
            }

            fn defaults(&self) -> crate::placeables::PlaceableDefaults {
                crate::placeables::PlaceableDefaults {
                    transform: crate::world::TransformData {
                        translation: [0.0_f32, 0.0_f32, 0.0_f32],
                        rotation_deg: [0.0_f32, 0.0_f32, 0.0_f32],
                        scale: [#scale_x, #scale_y, #scale_z],
                    },
                    tint: #tint_tokens,
                    collision: #collision_tokens,
                    blocks_movement: #blocks_movement,
                }
            }
        }

        impl crate::placeables::ResourceNodePlaceable for #name {
            fn resource_config(&self) -> crate::placeables::ResourceConfig {
                crate::placeables::ResourceConfig {
                    max_pieces: #max_pieces,
                    channel_seconds: #channel_seconds,
                    min_channel_seconds: #min_channel_seconds,
                    yield_item: crate::items::ItemId::new(#yield_item),
                    yield_amount: #yield_amount,
                    regen_interval_seconds: #regen_interval_seconds,
                    regen_amount: #regen_amount,
                    interact_range: #interact_range,
                    required_item_id: None,
                    bonus_tools: vec![#(#bonus_tools),*],
                }
            }
        }

        impl #name {
            pub const ID: &'static str = #id;
        }

        pub fn register(registry: &mut crate::placeables::PlaceableRegistry) {
            registry.register_resource(std::sync::Arc::new(#name))
        }
    })
}

/// Parsed `collision = ...` clause.
///
/// Kept as a small enum (instead of pre-rendering tokens) so that the macro
/// stays readable and so future shapes can be added in one place.
#[derive(Clone, Copy)]
enum CollisionSpec {
    None,
    Cylinder {
        radius: f32,
        height: f32,
    },
    Box {
        half_x: f32,
        half_y: f32,
        half_z: f32,
    },
    Sphere {
        radius: f32,
    },
}

/// Emits the Rust expression that constructs the `CollisionShape` (or `None`
/// when no collision was specified).
///
/// The output is used verbatim as the `collision:` field initializer in
/// `PlaceableDefaults`, so it must evaluate to `Option<CollisionShape>`.
fn build_collision_tokens(spec: CollisionSpec) -> proc_macro2::TokenStream {
    use quote::quote;

    let path = quote!(crate::world::CollisionShape);
    match spec {
        CollisionSpec::None => quote!(None),
        CollisionSpec::Cylinder { radius, height } => {
            quote!(Some(#path::Cylinder { radius: #radius, height: #height }))
        }
        CollisionSpec::Box {
            half_x,
            half_y,
            half_z,
        } => {
            quote!(Some(#path::Box { half_extents: [#half_x, #half_y, #half_z] }))
        }
        CollisionSpec::Sphere { radius } => {
            quote!(Some(#path::Sphere { radius: #radius }))
        }
    }
}

/// Simple attribute parser for syn 2.x compatibility
struct PropsAttributes {
    id: Option<String>,
    display_name: Option<String>,
    icon: Option<String>,
    asset: Option<String>,
    scale: Option<(f32, f32, f32)>,
    tint: Option<(f32, f32, f32)>,
    blocks_movement: bool,
    collision: CollisionSpec,
}

fn parse_attrs(input: &str) -> PropsAttributes {
    let mut props = PropsAttributes {
        id: None,
        display_name: None,
        icon: None,
        asset: None,
        scale: None,
        tint: None,
        blocks_movement: false,
        collision: CollisionSpec::None,
    };

    // The naive "split by comma" used previously breaks the moment the
    // `collision = ...` clause itself contains commas (e.g.
    // `cylinder(radius = 0.4, height = 6.0)`). To stay compatible with the
    // rest of the parser we split on top-level commas only, ignoring commas
    // nested inside parentheses.
    let pairs: Vec<String> = split_top_level_commas(input);

    for pair in pairs.iter() {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match key {
            "id" => props.id = Some(clean_string(value)),
            "name" => props.display_name = Some(clean_string(value)),
            "icon" => props.icon = Some(clean_string(value)),
            "asset" => props.asset = Some(clean_string(value)),
            "blocks_movement" => {
                props.blocks_movement = value == "true";
            }
            "scale" => {
                props.scale = parse_triple_f32(value);
            }
            "tint" => {
                props.tint = parse_triple_f32(value);
            }
            "collision" => {
                props.collision = parse_collision_spec(value);
            }
            _ => {}
        }
    }

    props
}

/// Splits the macro attribute string on commas that are *not* nested inside
/// parentheses. Required to support
/// `collision = cylinder(radius = 0.4, height = 6.0)` where the inner commas
/// belong to the collision DSL and must not split the key/value pair.
fn split_top_level_commas(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut current = String::new();
    for ch in input.chars() {
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            c => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Parses the collision DSL value (e.g. `"cylinder(radius = 0.4, height = 6.0)"`)
/// into a [`CollisionSpec`]. Returns [`CollisionSpec::None`] for unknown
/// shapes so a typo degrades to "no collision" instead of failing the build.
fn parse_collision_spec(value: &str) -> CollisionSpec {
    let value = value.trim();
    let Some((kind, body)) = value.split_once('(') else {
        return CollisionSpec::None;
    };
    let kind = kind.trim();
    // Strip the trailing ')'.
    let body = body.trim_end();
    let body = body.strip_suffix(')').unwrap_or(body);
    let fields = parse_kv_fields(body);

    match kind {
        "cylinder" => {
            let radius = fields.get("radius").and_then(|v| v.parse::<f32>().ok());
            let height = fields.get("height").and_then(|v| v.parse::<f32>().ok());
            match (radius, height) {
                (Some(radius), Some(height)) => CollisionSpec::Cylinder { radius, height },
                _ => CollisionSpec::None,
            }
        }
        "box" => {
            // Two accepted spellings: explicit half-extents triple, or
            // (width, height, depth) which we halve automatically.
            if let Some(half) = fields.get("half_extents").and_then(|s| parse_triple_f32(s)) {
                return CollisionSpec::Box {
                    half_x: half.0,
                    half_y: half.1,
                    half_z: half.2,
                };
            }
            let dims = parse_triple_f32(body);
            dims.map(|(x, y, z)| CollisionSpec::Box {
                half_x: x * 0.5,
                half_y: y * 0.5,
                half_z: z * 0.5,
            })
            .unwrap_or(CollisionSpec::None)
        }
        "sphere" => fields
            .get("radius")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|radius| CollisionSpec::Sphere { radius })
            .unwrap_or(CollisionSpec::None),
        _ => CollisionSpec::None,
    }
}

/// Parses a `key = value, key = value` body (without the surrounding parens)
/// into a small lookup table. Keys are case-sensitive and the last occurrence
/// of a duplicated key wins, matching the typical DSL behaviour.
fn parse_kv_fields(body: &str) -> HashMap<&str, &str> {
    body.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            let (k, v) = entry.split_once('=')?;
            Some((k.trim(), v.trim()))
        })
        .collect()
}

fn clean_string(s: &str) -> String {
    s.trim_matches('"').to_string()
}

/// Parses `[Axe]` / `[Axe, Hammer]` into variant names.
fn parse_ident_list(value: &str) -> Vec<String> {
    let inner = value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn parse_triple_f32(s: &str) -> Option<(f32, f32, f32)> {
    // Parse "(1.0, 2.0, 3.0)"
    let inner = s.trim_matches('(').trim_matches(')');
    let parts: Vec<&str> = inner.split(',').collect();

    if parts.len() == 3 {
        let x: f32 = parts[0].trim().parse().ok()?;
        let y: f32 = parts[1].trim().parse().ok()?;
        let z: f32 = parts[2].trim().parse().ok()?;
        Some((x, y, z))
    } else {
        None
    }
}

// ============================================================================
// #[item(...)] — declares a concrete `bevymmo_shared::items::Item`.
// ============================================================================
//
// Same spirit as `#[props(...)]` above (a unit struct + a small DSL generates
// the whole trait impl), but parsed with `syn::parse::Parse` instead of the
// manual string splitter, because the DSL here nests (`effects = [...]`,
// `spells(primary = [...], secondary = [...], ultimate = ...)`) and hand-rolled comma-splitting
// does not compose well past one level of nesting.
//
// Optionally declares the Primary/Secondary/Ultimate spell kit an item grants
// while equipped (`bevymmo_shared::items::SpellKit`):
// if the `spells(...)` clause is present, the macro rejects at compile time
// any shape that isn't "at least one Primary, at least one Secondary, exactly
// one Ultimate" — the contract required by the hotbar
// (`crate::spells::components::SpellHotbar`) — instead of only catching it at
// startup.
//
// # Example
// ```ignore
// use bevymmo_props_macro::item;
//
// // A weapon that grants two Primary options, one Secondary option, one Ultimate.
// #[item(
//     id = "magic_staff",
//     name = "Flame Staff",
//     description = "Forgiato nel cuore di un vulcano dormiente.",
//     category = Weapon,
//     rarity = Rare,
//     slot = Weapon,
//     icon = "items/icons/magic_staff.png",
//     tradable = true,
//     effects = [stat_bonus(field = AttackPower, op = Add, value = 25.0)],
//     spells(
//         primary = [AttackSpell, FireballSpell],
//         secondary = [StunFieldSpell],
//         ultimate = MeteoriteSpell,
//     ),
// )]
// pub struct MagicStaff;
//
// // A pure stat item — no `spells(...)` clause, `spell_kit()` stays `None`.
// #[item(
//     id = "iron_plate_armor_v2",
//     name = "Corazza di Ferro",
//     category = Armor,
//     rarity = Common,
//     slot = Armor,
//     effects = [stat_bonus(field = MaxHealth, op = Add, value = 500.0)],
// )]
// pub struct IronPlateArmorV2;
// ```
//
// Numeric values (`value = 25.0`, `amount = 10.0`) must be written as float
// literals with a decimal point, matching the rest of the codebase's
// convention for `f32` constants.

#[proc_macro_attribute]
pub fn item(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);

    // Only unit structs are supported: like `#[props(...)]`, all data comes
    // from macro arguments (stored in `static OnceLock`s in the generated
    // impl), not from struct fields.
    if let syn::Data::Struct(data) = &input.data {
        if !matches!(data.fields, syn::Fields::Unit) {
            return syn::Error::new_spanned(
                &input,
                "#[item(...)] can only be applied to a unit struct, e.g. `pub struct MagicStaff;` \
                 (all item data comes from the macro arguments, not from struct fields)",
            )
            .to_compile_error()
            .into();
        }
    } else {
        return syn::Error::new_spanned(&input, "#[item(...)] can only be applied to a struct")
            .to_compile_error()
            .into();
    }

    let def = parse_macro_input!(attr as ItemDef);
    TokenStream::from(def.build_tokens(&input))
}

/// One `key = value` pair inside `stat_bonus(...)` / `instant_heal(...)`.
/// The value is either a bare identifier (`field = MaxHealth`) or a float
/// literal (`value = 25.0`) — which one is expected depends on the key, so
/// [`KvPair::ident_value`] / [`KvPair::float_value`] check that at use time
/// and report a clear error if the wrong kind was written.
struct KvPair {
    key: Ident,
    value: KvValue,
}

enum KvValue {
    Ident(Ident),
    Float(LitFloat),
}

impl Parse for KvPair {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value = if input.peek(LitFloat) {
            KvValue::Float(input.parse()?)
        } else {
            KvValue::Ident(input.parse()?)
        };
        Ok(Self { key, value })
    }
}

impl KvPair {
    fn ident_value(&self) -> syn::Result<Ident> {
        match &self.value {
            KvValue::Ident(ident) => Ok(ident.clone()),
            KvValue::Float(lit) => Err(syn::Error::new_spanned(
                lit,
                format!("expected an identifier for `{}`, found a number", self.key),
            )),
        }
    }

    fn float_value(&self) -> syn::Result<LitFloat> {
        match &self.value {
            KvValue::Float(lit) => Ok(lit.clone()),
            KvValue::Ident(ident) => Err(syn::Error::new_spanned(
                ident,
                format!("expected a number for `{}`, found an identifier", self.key),
            )),
        }
    }
}

/// One entry of `effects = [...]`.
enum EffectDef {
    StatBonus {
        field: Ident,
        op: Ident,
        value: LitFloat,
    },
    InstantHeal {
        amount: LitFloat,
    },
}

impl Parse for EffectDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kind: Ident = input.parse()?;
        let content;
        parenthesized!(content in input);
        let pairs: Punctuated<KvPair, Token![,]> = Punctuated::parse_terminated(&content)?;

        match kind.to_string().as_str() {
            "stat_bonus" => {
                let mut field = None;
                let mut op = None;
                let mut value = None;
                for pair in &pairs {
                    match pair.key.to_string().as_str() {
                        "field" => field = Some(pair.ident_value()?),
                        "op" => op = Some(pair.ident_value()?),
                        "value" => value = Some(pair.float_value()?),
                        other => {
                            return Err(syn::Error::new_spanned(
                                &pair.key,
                                format!("unknown key `{other}` in stat_bonus(...) (expected field, op, value)"),
                            ))
                        }
                    }
                }
                Ok(EffectDef::StatBonus {
                    field: field
                        .ok_or_else(|| syn::Error::new_spanned(&kind, "stat_bonus(...) requires `field = ...`"))?,
                    op: op.ok_or_else(|| syn::Error::new_spanned(&kind, "stat_bonus(...) requires `op = ...`"))?,
                    value: value
                        .ok_or_else(|| syn::Error::new_spanned(&kind, "stat_bonus(...) requires `value = ...`"))?,
                })
            }
            "instant_heal" => {
                let mut amount = None;
                for pair in &pairs {
                    if pair.key == "amount" {
                        amount = Some(pair.float_value()?);
                    } else {
                        return Err(syn::Error::new_spanned(
                            &pair.key,
                            format!("unknown key `{}` in instant_heal(...) (expected amount)", pair.key),
                        ));
                    }
                }
                Ok(EffectDef::InstantHeal {
                    amount: amount
                        .ok_or_else(|| syn::Error::new_spanned(&kind, "instant_heal(...) requires `amount = ...`"))?,
                })
            }
            other => Err(syn::Error::new_spanned(
                &kind,
                format!("unknown effect `{other}` in effects = [...] (expected stat_bonus, instant_heal)"),
            )),
        }
    }
}

/// Parsed `abilities(primary = [...], secondary = [...], ultimate = [...])`
/// clause. Ogni slot deve offrire almeno una abilità; la selezione attiva vive
/// sull'esemplare e può includere anche l'Ultimate.
struct AbilitiesDef {
    primary: Vec<Path>,
    secondary: Vec<Path>,
    ultimate: Vec<Path>,
}

impl AbilitiesDef {
    fn parse_from(content: ParseStream) -> syn::Result<Self> {
        let mut primary: Option<Vec<Path>> = None;
        let mut secondary: Option<Vec<Path>> = None;
        let mut ultimate: Option<Vec<Path>> = None;

        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "primary" => {
                    let inner;
                    bracketed!(inner in content);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    primary = Some(list.into_iter().collect());
                }
                "secondary" => {
                    let inner;
                    bracketed!(inner in content);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    secondary = Some(list.into_iter().collect());
                }
                "ultimate" => {
                    let inner;
                    bracketed!(inner in content);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    ultimate = Some(list.into_iter().collect());
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in abilities(...) (expected primary, secondary, ultimate)"),
                    ))
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        let primary = primary.unwrap_or_default();
        let secondary = secondary.unwrap_or_default();

        if primary.is_empty() {
            return Err(syn::Error::new(
                content.span(),
                "abilities(...) requires at least one gesto in `primary = [...]`",
            ));
        }
        if secondary.is_empty() {
            return Err(syn::Error::new(
                content.span(),
                "abilities(...) requires at least one gesto in `secondary = [...]`",
            ));
        }
        let ultimate = ultimate.unwrap_or_default();
        if ultimate.is_empty() {
            return Err(syn::Error::new(
                content.span(),
                "abilities(...) requires at least one gesto in `ultimate = [...]`",
            ));
        }

        Ok(Self {
            primary,
            secondary,
            ultimate,
        })
    }
}

/// Parsed `rune_profile(capacity = ..., stability = ...)`.
/// Required alongside `abilities(...)` — a weapon senza profilo
/// runico non potrebbe mai essere incisa.
struct RuneProfileDef {
    capacity: LitInt,
    stability: LitFloat,
}

impl RuneProfileDef {
    fn parse_from(content: ParseStream) -> syn::Result<Self> {
        let mut capacity = None;
        let mut stability = None;

        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "capacity" => capacity = Some(content.parse::<LitInt>()?),
                "stability" => stability = Some(content.parse::<LitFloat>()?),

                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                        "unknown key `{other}` in rune_profile(...) (expected capacity, stability)"
                    ),
                    ))
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            capacity: capacity
                .ok_or_else(|| content.error("rune_profile(...) requires `capacity = ...`"))?,
            stability: stability
                .ok_or_else(|| content.error("rune_profile(...) requires `stability = ...`"))?,
        })
    }
}

/// Fully parsed `#[item(...)]` argument list.
struct CraftIngredientDef {
    id: LitStr,
    amount: LitInt,
}

impl Parse for CraftIngredientDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kind: Ident = input.parse()?;
        if kind != "ingredient" {
            return Err(syn::Error::new_spanned(
                &kind,
                "expected ingredient(id = \"...\", amount = N)",
            ));
        }
        let content;
        parenthesized!(content in input);
        let mut id = None;
        let mut amount = None;
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(content.parse::<LitStr>()?),
                "amount" => amount = Some(content.parse::<LitInt>()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in ingredient(...)"),
                    ))
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            id: id.ok_or_else(|| syn::Error::new_spanned(&kind, "ingredient(...) requires id"))?,
            amount: amount
                .ok_or_else(|| syn::Error::new_spanned(&kind, "ingredient(...) requires amount"))?,
        })
    }
}

struct CraftingDef {
    channel_seconds: LitFloat,
    ingredients: Vec<CraftIngredientDef>,
}

impl CraftingDef {
    fn parse_from(content: ParseStream) -> syn::Result<Self> {
        let mut channel_seconds = None;
        let mut ingredients = None;
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "channel_seconds" => channel_seconds = Some(content.parse::<LitFloat>()?),
                "ingredients" => {
                    let inner;
                    bracketed!(inner in content);
                    let list: Punctuated<CraftIngredientDef, Token![,]> =
                        Punctuated::parse_terminated(&inner)?;
                    ingredients = Some(list.into_iter().collect());
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "unknown key `{other}` in crafting(...) (expected channel_seconds, ingredients)"
                        ),
                    ))
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(Self {
            channel_seconds: channel_seconds
                .ok_or_else(|| content.error("crafting(...) requires `channel_seconds = ...`"))?,
            ingredients: ingredients
                .ok_or_else(|| content.error("crafting(...) requires `ingredients = [...]`"))?,
        })
    }
}

/// Fully parsed `#[item(...)]` argument list.
struct ItemDef {
    id: LitStr,
    name: LitStr,
    description: Option<LitStr>,
    category: Ident,
    rarity: Ident,
    slot: Option<Ident>,
    family: Option<Ident>,
    tradable: bool,
    icon: Option<LitStr>,
    effects: Vec<EffectDef>,
    abilities: Option<AbilitiesDef>,
    rune_profile: Option<RuneProfileDef>,
    crafting: Option<CraftingDef>,
    gathering_tool: Option<Ident>,
}

impl Parse for ItemDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut description = None;
        let mut category = None;
        let mut rarity = None;
        let mut slot = None;
        let mut family = None;
        let mut tradable = true;
        let mut icon = None;
        let mut effects = Vec::new();
        let mut abilities = None;
        let mut rune_profile = None;
        let mut crafting = None;
        let mut gathering_tool = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let key_str = key.to_string();

            if key_str == "abilities" {
                let content;
                parenthesized!(content in input);
                abilities = Some(AbilitiesDef::parse_from(&content)?);
            } else if key_str == "rune_profile" {
                let content;
                parenthesized!(content in input);
                rune_profile = Some(RuneProfileDef::parse_from(&content)?);
            } else if key_str == "crafting" {
                let content;
                parenthesized!(content in input);
                crafting = Some(CraftingDef::parse_from(&content)?);
            } else {
                input.parse::<Token![=]>()?;
                match key_str.as_str() {
                    "id" => id = Some(input.parse::<LitStr>()?),
                    "name" => name = Some(input.parse::<LitStr>()?),
                    "description" => description = Some(input.parse::<LitStr>()?),
                    "category" => category = Some(input.parse::<Ident>()?),
                    "rarity" => rarity = Some(input.parse::<Ident>()?),
                    "slot" => slot = Some(input.parse::<Ident>()?),
                    "family" => family = Some(input.parse::<Ident>()?),
                    "gathering_tool" => gathering_tool = Some(input.parse::<Ident>()?),
                    "tradable" => tradable = input.parse::<LitBool>()?.value(),
                    "icon" => icon = Some(input.parse::<LitStr>()?),
                    "effects" => {
                        let content;
                        bracketed!(content in input);
                        let list: Punctuated<EffectDef, Token![,]> = Punctuated::parse_terminated(&content)?;
                        effects = list.into_iter().collect();
                    }
                    other => {
                        return Err(syn::Error::new_spanned(
                            &key,
                            format!(
                                "unknown key `{other}` in #[item(...)] (expected id, name, description, \
                                 category, rarity, slot, family, gathering_tool, tradable, icon, effects, \
                                 abilities, rune_profile, crafting)"
                            ),
                        ))
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        if abilities.is_some() && rune_profile.is_none() {
            return Err(input.error(
                "#[item(...)] items with `abilities(...)` also need `rune_profile(...)` so the gestures can be inscribed",
            ));
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[item(...)] requires `id = \"...\"`"))?,
            name: name.ok_or_else(|| input.error("#[item(...)] requires `name = \"...\"`"))?,
            description,
            category: category.ok_or_else(|| {
                input.error(
                    "#[item(...)] requires `category = ...` (Weapon | Armor | Consumable | Material | Quest | Accessory | Tool)",
                )
            })?,
            rarity: rarity.ok_or_else(|| {
                input.error("#[item(...)] requires `rarity = ...` (Common | Uncommon | Rare | Epic | Legendary)")
            })?,
            slot,
            family,
            tradable,
            icon,
            effects,
            abilities,
            rune_profile,
            crafting,
            gathering_tool,
        })
    }
}

impl ItemDef {
    fn build_tokens(&self, original: &DeriveInput) -> TokenStream2 {
        let name = &original.ident;
        let id_lit = &self.id;
        let display_name_lit = &self.name;
        let description_lit = self
            .description
            .clone()
            .unwrap_or_else(|| LitStr::new("", proc_macro2::Span::call_site()));
        let category = &self.category;
        let rarity = &self.rarity;
        let tradable = self.tradable;
        let icon = icon_or(self.icon.clone(), empty_icon());
        let equippable_into = match &self.slot {
            Some(slot) => quote! { Some(crate::items::EquipSlot::#slot) },
            None => quote! { None },
        };
        let family_method = self.family.as_ref().map(|family| {
            let family_id = family.to_string().to_lowercase();
            quote! {
                fn weapon_family(&self) -> Option<crate::items::WeaponFamilyId> {
                    Some(crate::items::WeaponFamilyId::new(#family_id))
                }
            }
        });
        let gathering_tool_method = self.gathering_tool.as_ref().map(|kind| {
            quote! {
                fn gathering_tool(&self) -> Option<crate::items::GatheringToolKind> {
                    Some(crate::items::GatheringToolKind::#kind)
                }
            }
        });
        let effect_tokens: Vec<TokenStream2> = self
            .effects
            .iter()
            .map(|effect| match effect {
                EffectDef::StatBonus { field, op, value } => quote! {
                    crate::items::ItemEffect::StatBonus {
                        field: crate::stats::events::StatField::#field,
                        op: crate::stats::events::ModifierOp::#op,
                        value: #value,
                    }
                },
                EffectDef::InstantHeal { amount } => quote! {
                    crate::items::ItemEffect::InstantHeal { amount: #amount }
                },
            })
            .collect();

        // Items with `abilities(...)` expose a shared loadout. The same
        // generated method is usable by weapons and armor.
        let ability_loadout_method = self.abilities.as_ref().map(|abilities| {
            let primary = &abilities.primary;
            let secondary = &abilities.secondary;
            let ultimate = &abilities.ultimate;
            quote! {
                fn ability_loadout(&self) -> Option<&crate::abilities::AbilityLoadout> {
                    static ABILITIES: std::sync::OnceLock<crate::abilities::AbilityLoadout> = std::sync::OnceLock::new();
                    Some(ABILITIES.get_or_init(|| crate::abilities::AbilityLoadout::new(
                        vec![#(crate::abilities::AbilityId::new(#primary::ID)),*],
                        vec![#(crate::abilities::AbilityId::new(#secondary::ID)),*],
                        vec![#(crate::abilities::AbilityId::new(#ultimate::ID)),*],
                    )))
                }
            }
        });

        let rune_profile_method = self.rune_profile.as_ref().map(|profile| {
            let capacity = &profile.capacity;
            let stability = &profile.stability;
            quote! {
                fn rune_profile(&self) -> Option<&crate::abilities::RuneProfile> {
                    static PROFILE: std::sync::OnceLock<crate::abilities::RuneProfile> = std::sync::OnceLock::new();
                    Some(PROFILE.get_or_init(|| crate::abilities::RuneProfile {
                        capacity: #capacity,
                        stability: #stability,
                    }))
                }
            }
        });

        let craft_recipe_method = self.crafting.as_ref().map(|crafting| {
            let channel_seconds = &crafting.channel_seconds;
            let ingredients = crafting.ingredients.iter().map(|ingredient| {
                let id = &ingredient.id;
                let amount = &ingredient.amount;
                quote! {
                    crate::items::CraftIngredient {
                        item_id: crate::items::ItemId::new(#id),
                        amount: #amount,
                    }
                }
            });
            quote! {
                fn craft_recipe(&self) -> Option<&crate::items::CraftRecipe> {
                    static RECIPE: std::sync::OnceLock<crate::items::CraftRecipe> = std::sync::OnceLock::new();
                    Some(RECIPE.get_or_init(|| crate::items::CraftRecipe {
                        ingredients: vec![#(#ingredients),*],
                        channel_seconds: #channel_seconds,
                    }))
                }
            }
        });

        quote! {
            #original

            impl #name {
                /// Stable id, generated by `#[item(...)]` so it stays in sync
                /// with `id()` below — mirrors the `pub const ID` convention
                /// used by every hand-written item (e.g. `IronSword::ID`).
                pub const ID: &'static str = #id_lit;
            }

            impl crate::items::Item for #name {
                fn id(&self) -> crate::items::ItemId {
                    crate::items::ItemId::new(Self::ID)
                }

                fn config(&self) -> &crate::items::ItemConfig {
                    static CONFIG: std::sync::OnceLock<crate::items::ItemConfig> = std::sync::OnceLock::new();
                    CONFIG.get_or_init(|| crate::items::ItemConfig {
                        display_name: std::borrow::Cow::Borrowed(#display_name_lit),
                        description: std::borrow::Cow::Borrowed(#description_lit),
                        category: crate::items::ItemCategory::#category,
                        rarity: crate::items::ItemRarity::#rarity,
                        equippable_into: #equippable_into,
                        weight: 0.0,
                        tradable: #tradable,
                        icon: #icon,
                    })
                }

                fn effects(&self) -> &[crate::items::ItemEffect] {
                    static EFFECTS: std::sync::OnceLock<Vec<crate::items::ItemEffect>> = std::sync::OnceLock::new();
                    EFFECTS.get_or_init(|| vec![#(#effect_tokens),*])
                }

                #family_method
                #gathering_tool_method
                #ability_loadout_method
                #rune_profile_method
                #craft_recipe_method
            }

            impl #name {
                /// Registers this item in the global registry. Generated by `#[item(...)]`.
                pub fn register(registry: &mut crate::items::ItemRegistry) {
                    registry.register(std::sync::Arc::new(#name));
                }
            }
        }
    }
}

struct WeaponFamilyDef {
    id: LitStr,
    name: LitStr,
    icon: Option<LitStr>,
    primary: Option<Vec<Path>>,
    secondary: Option<Vec<Path>>,
    ultimate: Option<Vec<Path>>,
}

impl Parse for WeaponFamilyDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut icon = None;
        let mut primary = None;
        let mut secondary = None;
        let mut ultimate = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "icon" => icon = Some(input.parse::<LitStr>()?),
                "primary" => {
                    let inner;
                    bracketed!(inner in input);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    primary = Some(list.into_iter().collect());
                }
                "secondary" => {
                    let inner;
                    bracketed!(inner in input);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    secondary = Some(list.into_iter().collect());
                }
                "ultimate" => {
                    let inner;
                    bracketed!(inner in input);
                    let list: Punctuated<Path, Token![,]> = Punctuated::parse_terminated(&inner)?;
                    ultimate = Some(list.into_iter().collect());
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[weapon_family(...)] (expected id, name, icon, primary, secondary, ultimate)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[weapon_family(...)] requires `id = \"...\"`"))?,
            name: name
                .ok_or_else(|| input.error("#[weapon_family(...)] requires `name = \"...\"`"))?,
            icon,
            primary,
            secondary,
            ultimate,
        })
    }
}

#[proc_macro_attribute]
pub fn weapon_family(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "weapon_family") {
        return err;
    }
    let def = parse_macro_input!(attr as WeaponFamilyDef);
    let name = &input.ident;
    let id = &def.id;
    let display_name = &def.name;
    let icon = icon_or(def.icon, empty_icon());

    let ability_loadout = match (&def.primary, &def.secondary, &def.ultimate) {
        (Some(primary), Some(secondary), Some(ultimate))
            if !primary.is_empty() && !secondary.is_empty() && !ultimate.is_empty() =>
        {
            let expanded = quote! {
                Some(crate::abilities::AbilityLoadout::new(
                    vec![#(crate::abilities::AbilityId::new(#primary::ID)),*],
                    vec![#(crate::abilities::AbilityId::new(#secondary::ID)),*],
                    vec![#(crate::abilities::AbilityId::new(#ultimate::ID)),*],
                ))
            };
            Some(expanded)
        }
        _ => None,
    };

    let metadata_ability_loadout = match &ability_loadout {
        Some(tokens) => tokens.clone(),
        None => quote! { None },
    };

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id;

            pub fn metadata() -> crate::items::WeaponFamilyMetadata {
                <Self as crate::items::WeaponFamily>::metadata()
            }

            pub fn register(registry: &mut crate::items::WeaponFamilyRegistry) {
                registry.register(Self::metadata());
            }
        }

        impl crate::items::WeaponFamily for #name {
            fn metadata() -> crate::items::WeaponFamilyMetadata {
                crate::items::WeaponFamilyMetadata {
                    id: crate::items::WeaponFamilyId::new(Self::ID),
                    display_name: #display_name,
                    icon: #icon,
                    ability_loadout: #metadata_ability_loadout,
                }
            }
        }
    };

    TokenStream::from(expanded)
}

/// Optional `icon = "..."` path. `default` is used when the key is omitted
/// (`""` for most content; abilities default to `abilities/icons/{id}.png`).
fn icon_or(icon: Option<LitStr>, default: LitStr) -> LitStr {
    icon.unwrap_or(default)
}

fn empty_icon() -> LitStr {
    LitStr::new("", proc_macro2::Span::call_site())
}

fn default_ability_icon(id: &LitStr) -> LitStr {
    LitStr::new(&format!("abilities/icons/{}.png", id.value()), id.span())
}

/// Rejects anything but a bare unit struct — every macro in this file only
/// generates trait impls from literals, never from struct fields.
fn require_unit_struct(input: &DeriveInput, macro_name: &str) -> Result<(), TokenStream> {
    let ok =
        matches!(&input.data, syn::Data::Struct(data) if matches!(data.fields, syn::Fields::Unit));
    if ok {
        Ok(())
    } else {
        Err(syn::Error::new_spanned(
            input,
            format!(
                "#[{macro_name}(...)] can only be applied to a unit struct, e.g. `pub struct X;` \
                 (all data comes from the macro arguments, not from struct fields)"
            ),
        )
        .to_compile_error()
        .into())
    }
}

// ============================================================================
// #[base_ability(...)] — declares a `bevymmo_shared::abilities::BaseAbility`.
// ============================================================================
//
// Pure data, no behavior trait needed (unlike the three macros below):
// `BaseAbility::default_manifestation` already has a generic default derived
// from `geometry()`, so the macro can generate the ENTIRE `impl BaseAbility`.
//
// # Example
// ```ignore
// #[base_ability(
//     id = "staff_bolt", name = "Getto",
//     tags = [Ranged, Projectile, SingleTarget],
//     range = 20.0,
//     geometry = projectile(speed = 26.0),
//     potency = 260.0, cast_time = 0.35, cooldown = 4.0, mana_cost = 12.0,
//     animation = "staff_bolt_cast", impact_vfx = "bolt_impact_burst",
//     icon = "abilities/icons/staff_bolt.png",
// )]
// pub struct StaffBolt;
// ```

enum GeometryDef {
    Cone {
        radius: LitFloat,
        angle_deg: LitFloat,
    },
    /// `range` è la gittata entro cui si può piazzare il cerchio (se non specificata a livello radice).
    Circle {
        radius: LitFloat,
        range: Option<LitFloat>,
    },
    Projectile {
        speed: LitFloat,
        range: Option<LitFloat>,
    },
    SelfBuff {
        duration_seconds: LitFloat,
    },
}

fn parse_geometry(kind: Ident, fields: Punctuated<KvPair, Token![,]>) -> syn::Result<GeometryDef> {
    let field = |name: &str| fields.iter().find(|p| p.key == name);
    match kind.to_string().as_str() {
        "cone" => Ok(GeometryDef::Cone {
            radius: field("radius")
                .ok_or_else(|| syn::Error::new_spanned(&kind, "cone(...) requires `radius = ...`"))?
                .float_value()?,
            angle_deg: field("angle_deg")
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "cone(...) requires `angle_deg = ...`")
                })?
                .float_value()?,
        }),
        "circle" => Ok(GeometryDef::Circle {
            radius: field("radius")
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "circle(...) requires `radius = ...`")
                })?
                .float_value()?,
            range: field("range").map(|pair| pair.float_value()).transpose()?,
        }),
        "projectile" => Ok(GeometryDef::Projectile {
            speed: field("speed")
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "projectile(...) requires `speed = ...`")
                })?
                .float_value()?,
            range: field("range").map(|pair| pair.float_value()).transpose()?,
        }),
        "self_buff" => Ok(GeometryDef::SelfBuff {
            duration_seconds: field("duration_seconds")
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        &kind,
                        "self_buff(...) requires `duration_seconds = ...`",
                    )
                })?
                .float_value()?,
        }),
        other => Err(syn::Error::new_spanned(
            &kind,
            format!("unknown geometry `{other}` (expected cone, circle, projectile, self_buff)"),
        )),
    }
}

struct BaseAbilityDef {
    id: LitStr,
    name: LitStr,
    tags: Vec<Ident>,
    range: Option<LitFloat>,
    geometry: GeometryDef,
    potency: LitFloat,
    cast_time: LitFloat,
    cooldown: LitFloat,
    mana_cost: LitFloat,
    animation: LitStr,
    impact_vfx: LitStr,
    icon: Option<LitStr>,
    /// Opzionali: assenti = impatto immediato e nessun controllo.
    impact_delay: Option<LitFloat>,
    stun_seconds: Option<LitFloat>,
    statuses: Vec<Ident>,
    cleanse: Option<Ident>,
    /// Optional: "channeling" with tick_interval and movement_policy.
    /// Absent → derived from cast_time (positive = CastTime, zero = Instant).
    cast_mode: Option<CastModeDef>,
}

/// Parsed channeling configuration from the macro attributes.
struct CastModeDef {
    tick_interval: LitFloat,
    movement_policy: Ident, // InterruptOnMove or AllowMovement
}

impl Parse for BaseAbilityDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut tags = Vec::new();
        let mut range = None;
        let mut geometry = None;
        let mut potency = None;
        let mut cast_time = None;
        let mut cooldown = None;
        let mut mana_cost = None;
        let mut animation = None;
        let mut impact_vfx = None;
        let mut icon = None;
        let mut impact_delay = None;
        let mut stun_seconds = None;
        let mut statuses = Vec::new();
        let mut cleanse = None;
        let mut cast_mode = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let list: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
                    tags = list.into_iter().collect();
                }
                "range" => range = Some(input.parse::<LitFloat>()?),
                "geometry" => {
                    let kind: Ident = input.parse()?;
                    let content;
                    parenthesized!(content in input);
                    let fields: Punctuated<KvPair, Token![,]> = Punctuated::parse_terminated(&content)?;
                    geometry = Some(parse_geometry(kind, fields)?);
                }
                "potency" => potency = Some(input.parse::<LitFloat>()?),
                "cast_time" => cast_time = Some(input.parse::<LitFloat>()?),
                "cooldown" => cooldown = Some(input.parse::<LitFloat>()?),
                "mana_cost" => mana_cost = Some(input.parse::<LitFloat>()?),
                "animation" => animation = Some(input.parse::<LitStr>()?),
                "impact_vfx" => impact_vfx = Some(input.parse::<LitStr>()?),
                "icon" => icon = Some(input.parse::<LitStr>()?),
                "impact_delay" => impact_delay = Some(input.parse::<LitFloat>()?),
                "stun_seconds" => stun_seconds = Some(input.parse::<LitFloat>()?),
                "statuses" => {
                    let content;
                    bracketed!(content in input);
                    let list: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
                    statuses = list.into_iter().collect();
                }
                "cleanse" => cleanse = Some(input.parse::<Ident>()?),
                "channeling" => {
                    // Parse channeling(tick_interval = 0.25, movement = InterruptOnMove)
                    let content;
                    parenthesized!(content in input);
                    let fields: Punctuated<KvPair, Token![,]> = Punctuated::parse_terminated(&content)?;
                    let mut tick_interval = None;
                    let mut movement_policy = None;
                    for pair in fields {
                        match pair.key.to_string().as_str() {
                            "tick_interval" => {
                                tick_interval = Some(pair.float_value()?);
                            }
                            "movement" | "movement_policy" => {
                                movement_policy = Some(pair.ident_value()?);
                            }
                            _ => {} // Ignore unknown keys for forward compat.
                        }
                    }
                    let tick_interval = tick_interval.ok_or_else(||
                        syn::Error::new_spanned(&key, "channeling requires `tick_interval = ...`")
                    )?;
                    // Default movement policy if not specified.
                    let movement_policy = movement_policy.unwrap_or_else(|| {
                        syn::Ident::new("InterruptOnMove", proc_macro2::Span::call_site())
                    });
                    cast_mode = Some(CastModeDef { tick_interval, movement_policy });
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "unknown key `{other}` in #[base_ability(...)] (expected id, name, tags, range, geometry, \
                             potency, cast_time, cooldown, mana_cost, animation, impact_vfx, icon, impact_delay, \
                             stun_seconds, statuses, channeling)"
                        )
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[base_ability(...)] requires `id = \"...\"`"))?,
            name: name
                .ok_or_else(|| input.error("#[base_ability(...)] requires `name = \"...\"`"))?,
            tags,
            range,
            geometry: geometry
                .ok_or_else(|| input.error("#[base_ability(...)] requires `geometry = ...`"))?,
            potency: potency
                .ok_or_else(|| input.error("#[base_ability(...)] requires `potency = ...`"))?,
            cast_time: cast_time
                .ok_or_else(|| input.error("#[base_ability(...)] requires `cast_time = ...`"))?,
            cooldown: cooldown
                .ok_or_else(|| input.error("#[base_ability(...)] requires `cooldown = ...`"))?,
            mana_cost: mana_cost
                .ok_or_else(|| input.error("#[base_ability(...)] requires `mana_cost = ...`"))?,
            animation: animation.ok_or_else(|| {
                input.error("#[base_ability(...)] requires `animation = \"...\"`")
            })?,
            impact_vfx: impact_vfx.ok_or_else(|| {
                input.error("#[base_ability(...)] requires `impact_vfx = \"...\"`")
            })?,
            icon,
            impact_delay,
            stun_seconds,
            statuses,
            cleanse,
            cast_mode,
        })
    }
}

#[proc_macro_attribute]
pub fn base_ability(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "base_ability") {
        return err;
    }
    let def = parse_macro_input!(attr as BaseAbilityDef);
    let name = &input.ident;

    let id_lit = &def.id;
    let name_lit = &def.name;
    let tags = &def.tags;
    let potency = &def.potency;
    let cast_time = &def.cast_time;
    let cooldown = &def.cooldown;
    let mana_cost = &def.mana_cost;
    let animation = &def.animation;
    let impact_vfx = &def.impact_vfx;
    let icon = icon_or(def.icon, default_ability_icon(&def.id));

    let cleanse_method = match &def.cleanse {
        Some(filter) => {
            let filter = match filter.to_string().as_str() {
                "Buffs" => quote! { crate::effects::StatusFilter::Buffs },
                "Debuffs" => quote! { crate::effects::StatusFilter::Debuffs },
                "All" => quote! { crate::effects::StatusFilter::All },
                other => {
                    return syn::Error::new_spanned(
                        filter,
                        format!("unknown cleanse filter `{other}`; expected Buffs, Debuffs or All"),
                    )
                    .to_compile_error()
                    .into();
                }
            };
            quote! {
                fn cleanse_effect(&self) -> Option<crate::effects::CleanseEffect> {
                    Some(crate::effects::CleanseEffect {
                        filter: #filter,
                        max_statuses: None,
                        selection: crate::effects::StatusSelection::Newest,
                    })
                }
            }
        }
        None => quote! {},
    };

    let status_effects = def.statuses.iter().map(|status| {
        let status_id = syn::LitStr::new(&status.to_string().to_ascii_lowercase(), status.span());
        quote! {
            crate::effects::EffectSpec::ApplyStatus(crate::effects::ApplyStatusEffect {
                status_id: crate::effects::StatusId::new(#status_id),
                duration_override_seconds: None,
                potency: 1.0,
            })
        }
    });

    // `area`/`range` are derived from the geometry if not explicitly given.
    let (geometry_tokens, area, default_range) = match &def.geometry {
        GeometryDef::Cone { radius, angle_deg } => (
            quote! { crate::abilities::AbilityGeometry::Cone { radius: #radius, angle_deg: #angle_deg } },
            quote! { #radius },
            quote! { 0.0 },
        ),
        GeometryDef::Circle { radius, range } => {
            let range = match range {
                Some(range) => quote! { #range },
                None => quote! { 0.0 },
            };
            (
                quote! { crate::abilities::AbilityGeometry::Circle { radius: #radius } },
                quote! { #radius },
                range,
            )
        }
        GeometryDef::Projectile { speed, range } => {
            let range = match range {
                Some(range) => quote! { #range },
                None => quote! { 0.0 },
            };
            (
                quote! { crate::abilities::AbilityGeometry::Projectile { speed: #speed } },
                quote! { 0.0 },
                range,
            )
        }
        GeometryDef::SelfBuff { duration_seconds } => (
            quote! { crate::abilities::AbilityGeometry::SelfBuff { duration_seconds: #duration_seconds } },
            quote! { 0.0 },
            quote! { 0.0 },
        ),
    };

    let range = match &def.range {
        Some(r) => quote! { #r },
        None => default_range,
    };

    // Le due chiavi opzionali generano l'override solo se dichiarate: senza
    // di esse restano i default del trait (impatto immediato, nessuno stun).
    let impact_delay_method = match &def.impact_delay {
        Some(delay) => quote! {
            fn impact_delay(&self) -> f32 { #delay }
        },
        None => quote! {},
    };
    let stun_seconds_method = match &def.stun_seconds {
        Some(seconds) => quote! {
            fn stun_seconds(&self) -> f32 { #seconds }
        },
        None => quote! {},
    };

    // Cast mode override: if channeling is specified, generate cast_mode().
    // Otherwise the default trait method derives from cast_time.
    let cast_mode_method = match &def.cast_mode {
        Some(CastModeDef {
            tick_interval,
            movement_policy,
        }) => {
            let policy = match movement_policy.to_string().as_str() {
                "InterruptOnMove" => quote! { crate::abilities::ChannelMovementPolicy::InterruptOnMove },
                "AllowMovement" => quote! { crate::abilities::ChannelMovementPolicy::AllowMovement },
                other => {
                    return syn::Error::new_spanned(
                        movement_policy,
                        format!("unknown channel movement policy `{other}`; expected InterruptOnMove or AllowMovement")
                    ).to_compile_error().into()
                }
            };
            quote! {
                fn cast_mode(&self) -> crate::abilities::AbilityCastMode {
                    crate::abilities::AbilityCastMode::Channeling {
                        tick_interval_seconds: #tick_interval,
                        movement_policy: #policy,
                    }
                }
            }
        }
        None => quote! {}, // Use default: derived from cast_time
    };

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id_lit;
        }

        impl crate::abilities::BaseAbility for #name {
            fn id(&self) -> crate::abilities::AbilityId {
                crate::abilities::AbilityId::new(Self::ID)
            }
            fn display_name(&self) -> &'static str {
                #name_lit
            }
            fn tags(&self) -> &'static [crate::abilities::AbilityTag] {
                &[#(crate::abilities::AbilityTag::#tags),*]
            }
            fn geometry(&self) -> crate::abilities::AbilityGeometry {
                #geometry_tokens
            }
            fn base_params(&self) -> crate::abilities::AbilityParams {
                crate::abilities::AbilityParams {
                    potency: #potency,
                    area: #area,
                    range: #range,
                    cast_time: #cast_time,
                    cooldown: #cooldown,
                    mana_cost: #mana_cost,
                }
            }
            fn animation(&self) -> &'static str {
                #animation
            }
            fn impact_vfx(&self) -> &'static str {
                #impact_vfx
            }
            fn icon(&self) -> &'static str {
                #icon
            }
            #impact_delay_method
            #stun_seconds_method
            #cleanse_method
            fn additional_effects(&self) -> Vec<crate::effects::EffectSpec> {
                vec![#(#status_effects),*]
            }
            #cast_mode_method
        }

        impl #name {
            /// Registers this ability in the global registry. Generated by `#[base_ability(...)]`.
            pub fn register(registry: &mut crate::abilities::BaseAbilityRegistry) {
                registry.register(std::sync::Arc::new(#name));
            }
        }
    };

    TokenStream::from(expanded)
}

// ============================================================================
// #[essence(...)] / #[modifier(...)] / #[ancient_word(...)]
// ============================================================================
//
// All three share the same shape: metadata generated from literals, the
// actual effect logic delegated to a hand-written `*Effect` trait impl
// (`EssenceEffect`/`ModifierEffect`/`AncientWordEffect`) — see the module doc
// of `crates/shared/src/abilities/essence.rs` for why (an `impl` block can't
// be split in two, and the effect logic varies too much per Glifo to be a
// macro literal).

struct EssenceDef {
    id: LitStr,
    name: LitStr,
    rune_cost: LitInt,
    targets: Ident,
    color: (LitFloat, LitFloat, LitFloat),
}

fn parse_color_triple(input: ParseStream) -> syn::Result<(LitFloat, LitFloat, LitFloat)> {
    let content;
    parenthesized!(content in input);
    let values: Punctuated<LitFloat, Token![,]> = Punctuated::parse_terminated(&content)?;
    let values: Vec<_> = values.into_iter().collect();
    match values.as_slice() {
        [r, g, b] => Ok((r.clone(), g.clone(), b.clone())),
        _ => Err(syn::Error::new(
            content.span(),
            "color = (...) requires exactly 3 float components (r, g, b)",
        )),
    }
}

impl Parse for EssenceDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut rune_cost = None;
        let mut targets = None;
        let mut color = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "rune_cost" => rune_cost = Some(input.parse::<LitInt>()?),
                "targets" => targets = Some(input.parse::<Ident>()?),
                "color" => color = Some(parse_color_triple(input)?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[essence(...)] (expected id, name, rune_cost, targets, color)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[essence(...)] requires `id = \"...\"`"))?,
            name: name.ok_or_else(|| input.error("#[essence(...)] requires `name = \"...\"`"))?,
            rune_cost: rune_cost
                .ok_or_else(|| input.error("#[essence(...)] requires `rune_cost = ...`"))?,
            targets: targets.ok_or_else(|| {
                input.error("#[essence(...)] requires `targets = allies | enemies`")
            })?,
            color: color
                .ok_or_else(|| input.error("#[essence(...)] requires `color = (r, g, b)`"))?,
        })
    }
}

#[proc_macro_attribute]
pub fn essence(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "essence") {
        return err;
    }
    let def = parse_macro_input!(attr as EssenceDef);
    let name = &input.ident;

    let id_lit = &def.id;
    let name_lit = &def.name;
    let rune_cost = &def.rune_cost;
    let (r, g, b) = &def.color;

    // §13 of the design: no dedicated "who" Glyph — allies/enemies is a
    // built-in rule of the Essence, expressed with the caster-relative
    // `AoeTargeting` the engine already has (mirrors HealingCircle/Meteorite).
    let targeting_tokens = match def.targets.to_string().as_str() {
        "allies" => quote! { crate::spells::context::AoeTargeting::CasterOnly },
        "enemies" => quote! { crate::spells::context::AoeTargeting::ExcludeCaster },
        other => {
            return syn::Error::new_spanned(
                &def.targets,
                format!(
                    "unknown `targets = {other}` in #[essence(...)] (expected allies or enemies)"
                ),
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id_lit;
        }

        impl crate::abilities::Essence for #name {
            fn id(&self) -> crate::abilities::EssenceId {
                crate::abilities::EssenceId::new(Self::ID)
            }
            fn display_name(&self) -> &'static str {
                #name_lit
            }
            fn rune_cost(&self) -> u32 {
                #rune_cost
            }
            fn default_targeting(&self) -> crate::spells::context::AoeTargeting {
                #targeting_tokens
            }
            fn visual_theme(&self) -> crate::abilities::EssenceVisualTheme {
                crate::abilities::EssenceVisualTheme {
                    color: crate::math::Rgba::opaque(#r, #g, #b),
                }
            }
            fn manifest(
                &self,
                ability: &dyn crate::abilities::BaseAbility,
                params: &crate::abilities::AbilityParams,
                ctx: &mut crate::spells::context::SpellCastContext,
            ) {
                <Self as crate::abilities::EssenceEffect>::manifest(self, ability, params, ctx)
            }
        }

        impl #name {
            /// Registers this Essenza in the global registry. Generated by `#[essence(...)]`.
            pub fn register(registry: &mut crate::abilities::EssenceRegistry) {
                registry.register(std::sync::Arc::new(#name));
            }
        }
    };

    TokenStream::from(expanded)
}

struct ModifierDef {
    id: LitStr,
    name: LitStr,
    tag: Ident,
    rune_cost: LitInt,
}

impl Parse for ModifierDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut tag = None;
        let mut rune_cost = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "tag" => tag = Some(input.parse::<Ident>()?),
                "rune_cost" => rune_cost = Some(input.parse::<LitInt>()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[modifier(...)] (expected id, name, tag, rune_cost)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[modifier(...)] requires `id = \"...\"`"))?,
            name: name.ok_or_else(|| input.error("#[modifier(...)] requires `name = \"...\"`"))?,
            tag: tag.ok_or_else(|| {
                input.error("#[modifier(...)] requires `tag = ...` (an AbilityTag)")
            })?,
            rune_cost: rune_cost
                .ok_or_else(|| input.error("#[modifier(...)] requires `rune_cost = ...`"))?,
        })
    }
}

#[proc_macro_attribute]
pub fn modifier(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "modifier") {
        return err;
    }
    let def = parse_macro_input!(attr as ModifierDef);
    let name = &input.ident;

    let id_lit = &def.id;
    let name_lit = &def.name;
    let tag = &def.tag;
    let rune_cost = &def.rune_cost;

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id_lit;
        }

        impl crate::abilities::Modifier for #name {
            fn id(&self) -> crate::abilities::ModifierId {
                crate::abilities::ModifierId::new(Self::ID)
            }
            fn display_name(&self) -> &'static str {
                #name_lit
            }
            fn required_tag(&self) -> crate::abilities::AbilityTag {
                crate::abilities::AbilityTag::#tag
            }
            fn rune_cost(&self) -> u32 {
                #rune_cost
            }
            fn transform(&self, params: &mut crate::abilities::AbilityParams) {
                <Self as crate::abilities::ModifierEffect>::transform(self, params)
            }
        }

        impl #name {
            /// Registers this Modificatore in the global registry. Generated by `#[modifier(...)]`.
            pub fn register(registry: &mut crate::abilities::ModifierRegistry) {
                registry.register(std::sync::Arc::new(#name));
            }
        }
    };

    TokenStream::from(expanded)
}

struct PeriodicDef {
    interval: LitFloat,
    amount: LitFloat,
}

struct StatusModifierDef {
    stat: Ident,
    operation: Ident,
    value: LitFloat,
}

struct StatusDef {
    id: LitStr,
    name: Option<LitStr>,
    icon: Option<LitStr>,
    category: Ident,
    duration: LitFloat,
    cleanseable: bool,
    purgeable: bool,
    stacking: Ident,
    stack_scope: Ident,
    max_stacks: LitInt,
    refresh: Ident,
    control: Option<Ident>,
    periodic: Option<PeriodicDef>,
    modifier: Option<StatusModifierDef>,
}

impl Parse for StatusDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut icon = None;
        let mut category = None;
        let mut duration = None;
        let mut cleanseable = false;
        let mut purgeable = false;
        let mut stacking = Ident::new("None", proc_macro2::Span::call_site());
        let mut stack_scope = Ident::new("Global", proc_macro2::Span::call_site());
        let mut max_stacks = syn::parse_str::<LitInt>("1").expect("valid integer literal");
        let mut refresh = Ident::new("None", proc_macro2::Span::call_site());
        let mut control = None;
        let mut periodic = None;
        let mut modifier = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key == "modifier" {
                let content;
                parenthesized!(content in input);
                let mut stat = None;
                let mut operation = None;
                let mut value = None;
                while !content.is_empty() {
                    let nested_key: Ident = content.parse()?;
                    content.parse::<Token![=]>()?;
                    match nested_key.to_string().as_str() {
                        "stat" => stat = Some(content.parse::<Ident>()?),
                        "operation" => operation = Some(content.parse::<Ident>()?),
                        "value" => value = Some(content.parse::<LitFloat>()?),
                        other => {
                            return Err(syn::Error::new_spanned(
                                &nested_key,
                                format!("unknown key `{other}` in modifier(...) (expected stat, operation, value)"),
                            ))
                        }
                    }
                    if content.peek(Token![,]) {
                        content.parse::<Token![,]>()?;
                    } else {
                        break;
                    }
                }
                modifier = Some(StatusModifierDef {
                    stat: stat.ok_or_else(|| input.error("modifier(...) requires `stat = ...`"))?,
                    operation: operation
                        .ok_or_else(|| input.error("modifier(...) requires `operation = ...`"))?,
                    value: value
                        .ok_or_else(|| input.error("modifier(...) requires `value = ...`"))?,
                });
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            if key == "periodic" {
                let content;
                parenthesized!(content in input);
                let mut interval = None;
                let mut amount = None;
                while !content.is_empty() {
                    let nested_key: Ident = content.parse()?;
                    content.parse::<Token![=]>()?;
                    match nested_key.to_string().as_str() {
                        "interval" => interval = Some(content.parse::<LitFloat>()?),
                        "amount" => amount = Some(content.parse::<LitFloat>()?),
                        other => {
                            return Err(syn::Error::new_spanned(
                                &nested_key,
                                format!("unknown key `{other}` in periodic(...) (expected interval, amount)"),
                            ))
                        }
                    }
                    if content.peek(Token![,]) {
                        content.parse::<Token![,]>()?;
                    } else {
                        break;
                    }
                }
                periodic = Some(PeriodicDef {
                    interval: interval
                        .ok_or_else(|| input.error("periodic(...) requires `interval = ...`"))?,
                    amount: amount
                        .ok_or_else(|| input.error("periodic(...) requires `amount = ...`"))?,
                });
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "icon" => icon = Some(input.parse::<LitStr>()?),
                "category" => category = Some(input.parse::<Ident>()?),
                "duration" => duration = Some(input.parse::<LitFloat>()?),
                "cleanseable" => cleanseable = input.parse::<LitBool>()?.value,
                "purgeable" => purgeable = input.parse::<LitBool>()?.value,
                "stacking" => stacking = input.parse::<Ident>()?,
                "stack_scope" => stack_scope = input.parse::<Ident>()?,
                "max_stacks" => max_stacks = input.parse::<LitInt>()?,
                "refresh" => refresh = input.parse::<Ident>()?,
                "control" => control = Some(input.parse::<Ident>()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[status(...)] (expected id, category, duration, cleanseable, stacking, stack_scope, max_stacks, refresh, control)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[status(...)] requires `id = \"...\"`"))?,
            name,
            icon,
            category: category
                .ok_or_else(|| input.error("#[status(...)] requires `category = Buff|Debuff`"))?,
            duration: duration
                .ok_or_else(|| input.error("#[status(...)] requires `duration = ...`"))?,
            cleanseable,
            purgeable,
            stacking,
            stack_scope,
            max_stacks,
            refresh,
            control,
            periodic,
            modifier,
        })
    }
}

#[proc_macro_attribute]
pub fn status(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "status") {
        return err;
    }
    let def = parse_macro_input!(attr as StatusDef);
    let type_name = &input.ident;
    let id = &def.id;
    let display_name = def.name.unwrap_or_else(|| id.clone());
    let icon = def
        .icon
        .unwrap_or_else(|| LitStr::new("status_default", id.span()));
    let category = &def.category;
    let duration = &def.duration;
    let cleanseable = def.cleanseable;
    let purgeable = def.purgeable;
    let stacking = &def.stacking;
    let stack_scope = &def.stack_scope;
    let max_stacks = &def.max_stacks;
    let refresh = &def.refresh;
    let periodic = match def.periodic {
        Some(periodic) => {
            let interval = periodic.interval;
            let amount = periodic.amount;
            quote!(Some(crate::effects::PeriodicSpec {
                interval_seconds: #interval,
                effect: crate::effects::PeriodicEffect::Damage {
                    amount: #amount,
                },
            }))
        }
        None => quote!(None),
    };
    let stat_modifiers = match def.modifier {
        Some(modifier) => {
            let stat = modifier.stat;
            let operation = modifier.operation;
            let value = modifier.value;
            quote!(&[crate::effects::StatModifierSpec {
                field: crate::stats::events::StatField::#stat,
                operation: crate::stats::events::ModifierOp::#operation,
                value: #value,
            }])
        }
        None => quote!(&[]),
    };
    let dispel = if cleanseable {
        quote!(crate::effects::DispelPolicy::RemoveWholeStatus)
    } else {
        quote!(crate::effects::DispelPolicy::NotDispellable)
    };
    let control = match def.control {
        Some(control) => quote!(Some(crate::effects::ControlSpec::#control)),
        None => quote!(None),
    };

    let expanded = quote! {
        #input

        impl #type_name {
            pub const ID: &'static str = #id;
        }

        impl crate::effects::Status for #type_name {
            fn definition() -> crate::effects::StatusDefinition {
                crate::effects::StatusDefinition {
                    id: crate::effects::StatusId::new(Self::ID),
                    category: crate::effects::StatusCategory::#category,
                    duration_seconds: #duration,
                    cleanseable: #cleanseable,
                    purgeable: #purgeable,
                    stacking: crate::effects::StackPolicy::#stacking,
                    stack_scope: crate::effects::StackScope::#stack_scope,
                    max_stacks: #max_stacks,
                    refresh: crate::effects::RefreshPolicy::#refresh,
                    dispel: #dispel,
                    periodic: #periodic,
                    stat_modifiers: #stat_modifiers,
                    control: #control,
                    presentation: crate::effects::StatusPresentation {
                        icon: #icon,
                        short_name: #display_name,
                    },
                }
            }
        }

        impl #type_name {
            pub fn status_id() -> crate::effects::StatusId {
                <Self as crate::effects::Status>::status_id()
            }

            pub fn register(registry: &mut crate::effects::StatusRegistry) {
                registry.register(<Self as crate::effects::Status>::definition());
            }
        }
    };

    TokenStream::from(expanded)
}

struct AncientWordDef {
    id: LitStr,
    name: LitStr,
    icon: Option<LitStr>,
    tag: Ident,
    /// Full required-tag list when `tags = [...]` is set; otherwise just `tag`.
    required_tags: Vec<Ident>,
    rune_cost: LitInt,
}

impl Parse for AncientWordDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut icon = None;
        let mut tag = None;
        let mut required_tags = Vec::new();
        let mut rune_cost = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "icon" => icon = Some(input.parse::<LitStr>()?),
                "tag" => tag = Some(input.parse::<Ident>()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let list: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
                    required_tags = list.into_iter().collect();
                }
                "rune_cost" => rune_cost = Some(input.parse::<LitInt>()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[ancient_word(...)] (expected id, name, icon, tag, tags, rune_cost)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[ancient_word(...)] requires `id = \"...\"`"))?,
            name: name
                .ok_or_else(|| input.error("#[ancient_word(...)] requires `name = \"...\"`"))?,
            icon,
            tag: tag.ok_or_else(|| {
                input.error("#[ancient_word(...)] requires `tag = ...` (an AbilityTag)")
            })?,
            required_tags,
            rune_cost: rune_cost
                .ok_or_else(|| input.error("#[ancient_word(...)] requires `rune_cost = ...`"))?,
        })
    }
}

#[proc_macro_attribute]
pub fn ancient_word(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "ancient_word") {
        return err;
    }
    let def = parse_macro_input!(attr as AncientWordDef);
    let name = &input.ident;

    let id_lit = &def.id;
    let name_lit = &def.name;
    let icon = icon_or(def.icon, empty_icon());
    let tag = &def.tag;
    let rune_cost = &def.rune_cost;
    let required_tags = if def.required_tags.is_empty() {
        vec![tag.clone()]
    } else {
        def.required_tags.clone()
    };

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id_lit;
        }

        impl crate::abilities::AncientWord for #name {
            fn id(&self) -> crate::abilities::AncientWordId {
                crate::abilities::AncientWordId::new(Self::ID)
            }
            fn display_name(&self) -> &'static str {
                #name_lit
            }
            fn required_tag(&self) -> crate::abilities::AbilityTag {
                crate::abilities::AbilityTag::#tag
            }
            fn rune_cost(&self) -> u32 {
                #rune_cost
            }
            fn metadata(&self) -> crate::abilities::AncientWordMetadata {
                crate::abilities::AncientWordMetadata {
                    display_name: #name_lit,
                    required_tags: vec![#(crate::abilities::AbilityTag::#required_tags),*],
                    forbidden_tags: Vec::new(),
                    exclusive_group: None,
                    phase: 0,
                    visual_priority: 0,
                    rune_cost: #rune_cost,
                    icon: #icon,
                }
            }
            fn post_process(
                &self,
                ability: &dyn crate::abilities::BaseAbility,
                params: &crate::abilities::AbilityParams,
                ctx: &mut crate::spells::context::SpellCastContext,
            ) {
                <Self as crate::abilities::AncientWordEffect>::post_process(self, ability, params, ctx)
            }
            fn transform_blueprint(&self, blueprint: &mut crate::abilities::AbilityBlueprint) {
                <Self as crate::abilities::AncientWordEffect>::transform_blueprint(self, blueprint)
            }
        }

        impl #name {
            /// Registers this Parola Antica in the global registry. Generated by `#[ancient_word(...)]`.
            pub fn register(registry: &mut crate::abilities::AncientWordRegistry) {
                registry.register(std::sync::Arc::new(#name));
            }
        }
    };

    TokenStream::from(expanded)
}

// ============================================================================
// #[spell(...)] — declares a `bevymmo_domain::spells::Spell`.
// ============================================================================
//
// Generates the static metadata (id/display_name/config/register) following
// the same pattern as #[base_ability], #[essence], and #[modifier].
// The actual cast logic is left to the user via a separate `SpellCast` trait
// impl — same delegation used by EssenceEffect / ModifierEffect / AncientWordEffect.
//
struct RootWordDef {
    id: LitStr,
    name: LitStr,
    description: LitStr,
    rune_cost: LitInt,
    icon: Option<LitStr>,
}

impl Parse for RootWordDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut name = None;
        let mut description = None;
        let mut rune_cost = None;
        let mut icon = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(input.parse::<LitStr>()?),
                "name" => name = Some(input.parse::<LitStr>()?),
                "description" => description = Some(input.parse::<LitStr>()?),
                "rune_cost" => rune_cost = Some(input.parse::<LitInt>()?),
                "icon" => icon = Some(input.parse::<LitStr>()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in #[root_word(...)] (expected id, name, description, rune_cost, icon)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[root_word(...)] requires `id = \"...\"`"))?,
            name: name.ok_or_else(|| input.error("#[root_word(...)] requires `name = \"...\"`"))?,
            description: description
                .ok_or_else(|| input.error("#[root_word(...)] requires `description = \"...\"`"))?,
            rune_cost: rune_cost
                .ok_or_else(|| input.error("#[root_word(...)] requires `rune_cost = ...`"))?,
            icon,
        })
    }
}

#[proc_macro_attribute]
pub fn root_word(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "root_word") {
        return err;
    }
    let def = parse_macro_input!(attr as RootWordDef);
    let name = &input.ident;

    let id_lit = &def.id;
    let name_lit = &def.name;
    let description_lit = &def.description;
    let rune_cost = &def.rune_cost;
    let icon = icon_or(def.icon, empty_icon());

    let expanded = quote! {
        #input

        impl #name {
            pub const ID: &'static str = #id_lit;
        }

        impl crate::abilities::RootWord for #name {
            fn id(&self) -> crate::abilities::RootWordId {
                crate::abilities::RootWordId::from(Self::ID)
            }
            fn metadata(&self) -> &crate::abilities::RootWordMetadata {
                static META: crate::abilities::RootWordMetadata = crate::abilities::RootWordMetadata {
                    display_name: #name_lit,
                    description: #description_lit,
                    rune_cost: #rune_cost,
                    icon: #icon,
                };
                &META
            }
            fn apply_to_blueprint(
                &self,
                blueprint: &mut crate::abilities::AbilityBlueprint,
                params: &crate::abilities::AbilityParams,
            ) {
                <Self as crate::abilities::RootWordEffect>::apply_to_blueprint(self, blueprint, params)
            }
        }

        impl #name {
            /// Registers this Root Word in the global registry. Generated by `#[root_word(...)]`.
            pub fn register(registry: &mut crate::abilities::RootWordRegistry) {
                registry.register(std::sync::Arc::new(#name));
            }
        }
    };

    TokenStream::from(expanded)
}

// ============================================================================
// #[enemy(...)] — declares a catalog enemy (Normal) or boss (Boss).
// ============================================================================
//
// Parsed with `syn::parse::Parse` like `#[item(...)]` because the DSL nests
// (`stats(...)`, `abilities = [Cleave(when = (...), root = Flame)]`,
// `phases = [(id = "...", ...)]`).
//
// # Example
// ```ignore
// #[enemy(
//     id = "mob_goblin",
//     type = Normal,
//     name = "Goblin",
//     icon = "👺",
//     asset = "models/creatures/goblin.glb",
//     stats(health = 30.0, armor = 8.0),
//     aggro = 8.0,
//     leash_aggro = 20.0,
//     abilities = [Cleave],
// )]
// pub struct Goblin;
// ```

#[proc_macro_attribute]
pub fn enemy(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    if let Err(err) = require_unit_struct(&input, "enemy") {
        return err;
    }
    let def = parse_macro_input!(attr as EnemyDef);
    match def.build_tokens(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[derive(Clone, Copy)]
enum EnemyKind {
    Normal,
    Boss,
}

struct EnemyStatsDef {
    health: Option<LitFloat>,
    mana: Option<LitFloat>,
    mana_regen: Option<LitFloat>,
    attack_power: Option<LitFloat>,
    armor: Option<LitFloat>,
    speed: Option<LitFloat>,
}

impl EnemyStatsDef {
    fn parse_from(content: ParseStream) -> syn::Result<Self> {
        let mut stats = Self {
            health: None,
            mana: None,
            mana_regen: None,
            attack_power: None,
            armor: None,
            speed: None,
        };
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            let value = parse_number_f32(content)?;
            match key.to_string().as_str() {
                "health" => stats.health = Some(value),
                "mana" => stats.mana = Some(value),
                "mana_regen" => stats.mana_regen = Some(value),
                "attack_power" => stats.attack_power = Some(value),
                "armor" => stats.armor = Some(value),
                "speed" => stats.speed = Some(value),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "unknown stats field `{other}` (expected health, mana, mana_regen, \
                             attack_power, armor, speed)"
                        ),
                    ));
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(stats)
    }

    fn override_tokens(&self) -> Vec<TokenStream2> {
        let mut out = Vec::new();
        if let Some(health) = &self.health {
            out.push(quote! {
                stats.vital.current_health = #health;
                stats.vital.max_health = #health;
            });
        }
        if let Some(mana) = &self.mana {
            out.push(quote! {
                stats.vital.current_mana = #mana;
                stats.vital.max_mana = #mana;
            });
        }
        if let Some(mana_regen) = &self.mana_regen {
            out.push(quote! {
                stats.vital.mana_regeneration = #mana_regen;
            });
        }
        if let Some(attack_power) = &self.attack_power {
            out.push(quote! {
                stats.combat.attack_power = #attack_power;
            });
        }
        if let Some(armor) = &self.armor {
            out.push(quote! {
                stats.combat.armor = #armor;
            });
        }
        if let Some(speed) = &self.speed {
            out.push(quote! {
                stats.movement.speed = #speed;
            });
        }
        out
    }
}

enum AbilityTargetingDef {
    Main,
    Farthest,
    SelfCentered,
    DensestCluster { n: LitInt },
}

impl AbilityTargetingDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Token![Self]) {
            input.parse::<Token![Self]>()?;
            return Ok(Self::SelfCentered);
        }
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "Main" => Ok(Self::Main),
            "Farthest" => Ok(Self::Farthest),
            "Self" | "SelfCentered" => Ok(Self::SelfCentered),
            "Cluster" | "DensestCluster" => {
                let content;
                parenthesized!(content in input);
                let n: LitInt = content.parse()?;
                if !content.is_empty() {
                    return Err(content.error("Cluster(n) takes a single integer"));
                }
                Ok(Self::DensestCluster { n })
            }
            other => Err(syn::Error::new_spanned(
                &ident,
                format!("unknown targeting `{other}` (expected Main, Farthest, Self, Cluster(n))"),
            )),
        }
    }

    fn to_tokens(&self) -> TokenStream2 {
        match self {
            Self::Main => quote!(crate::placeables::AbilityTargeting::Main),
            Self::Farthest => quote!(crate::placeables::AbilityTargeting::Farthest),
            Self::SelfCentered => quote!(crate::placeables::AbilityTargeting::SelfCentered),
            Self::DensestCluster { n } => {
                quote!(crate::placeables::AbilityTargeting::DensestCluster { n: #n })
            }
        }
    }
}

struct AbilityWhenDef {
    interval: Option<LitFloat>,
    min_range: Option<LitFloat>,
    max_range: Option<LitFloat>,
    hp_above: Option<LitFloat>,
    hp_below: Option<LitFloat>,
    priority: Option<LitInt>,
    targeting: Option<AbilityTargetingDef>,
}

impl AbilityWhenDef {
    fn parse_from(content: ParseStream) -> syn::Result<Self> {
        let mut when = Self {
            interval: None,
            min_range: None,
            max_range: None,
            hp_above: None,
            hp_below: None,
            priority: None,
            targeting: None,
        };
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "interval" => when.interval = Some(parse_number_f32(content)?),
                "min_range" => when.min_range = Some(parse_number_f32(content)?),
                "max_range" => when.max_range = Some(parse_number_f32(content)?),
                "hp_above" => when.hp_above = Some(parse_number_f32(content)?),
                "hp_below" => when.hp_below = Some(parse_number_f32(content)?),
                "priority" => when.priority = Some(content.parse::<LitInt>()?),
                "targeting" => when.targeting = Some(AbilityTargetingDef::parse(content)?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "unknown key `{other}` in when(...) (expected interval, min_range, \
                             max_range, hp_above, hp_below, priority, targeting)"
                        ),
                    ));
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(when)
    }

    fn to_tokens(&self) -> TokenStream2 {
        let interval = self.interval.clone().unwrap_or_else(|| lit_f32("0.0"));
        let min_range = self.min_range.clone().unwrap_or_else(|| lit_f32("0.0"));
        let max_range = match &self.max_range {
            Some(value) => quote!(Some(#value)),
            None => quote!(None),
        };
        let hp_above = self.hp_above.clone().unwrap_or_else(|| lit_f32("0.0"));
        let hp_below = self.hp_below.clone().unwrap_or_else(|| lit_f32("1.0"));
        let priority = self
            .priority
            .clone()
            .unwrap_or_else(|| LitInt::new("0", proc_macro2::Span::call_site()));
        let targeting = self
            .targeting
            .as_ref()
            .map(AbilityTargetingDef::to_tokens)
            .unwrap_or_else(|| quote!(crate::placeables::AbilityTargeting::Main));
        quote! {
            crate::placeables::AbilityUse {
                interval: #interval,
                min_range: #min_range,
                max_range: #max_range,
                hp_above: #hp_above,
                hp_below: #hp_below,
                priority: #priority,
                targeting: #targeting,
            }
        }
    }
}

struct AbilityEntryDef {
    path: Path,
    root: Option<Path>,
    when: Option<AbilityWhenDef>,
}

impl Parse for AbilityEntryDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: Path = input.parse()?;
        let mut root = None;
        let mut when = None;
        if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            while !content.is_empty() {
                let key: Ident = content.parse()?;
                content.parse::<Token![=]>()?;
                match key.to_string().as_str() {
                    "root" => root = Some(content.parse::<Path>()?),
                    "when" => {
                        let inner;
                        parenthesized!(inner in content);
                        when = Some(AbilityWhenDef::parse_from(&inner)?);
                    }
                    other => {
                        return Err(syn::Error::new_spanned(
                            &key,
                            format!("unknown key `{other}` in ability entry (expected root, when)"),
                        ));
                    }
                }
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
        }
        Ok(Self { path, root, when })
    }
}

impl AbilityEntryDef {
    fn to_tokens(&self) -> TokenStream2 {
        let path = &self.path;
        let inscription = match &self.root {
            Some(root) => quote! {
                crate::abilities::KitInscription {
                    root_word: Some(crate::abilities::RootWordId::from(#root::ID)),
                    secondary_words: ::std::vec::Vec::new(),
                }
            },
            None => quote!(crate::abilities::KitInscription::default()),
        };
        let use_when = match &self.when {
            Some(when) => when.to_tokens(),
            None => quote!(crate::placeables::AbilityUse::default()),
        };
        quote! {
            crate::placeables::AbilityKitEntry {
                ability_id: crate::abilities::AbilityId::new(#path::ID),
                inscription: #inscription,
                use_when: #use_when,
            }
        }
    }
}

enum CombatMovementDef {
    Chase,
    KeepDistance { range: LitFloat },
    Stationary,
    Hover,
}

impl CombatMovementDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "Chase" => Ok(Self::Chase),
            "Stationary" => Ok(Self::Stationary),
            "Hover" => Ok(Self::Hover),
            "KeepDistance" => {
                let content;
                parenthesized!(content in input);
                let mut range = None;
                while !content.is_empty() {
                    let key: Ident = content.parse()?;
                    content.parse::<Token![=]>()?;
                    if key == "range" {
                        range = Some(parse_number_f32(&content)?);
                    } else {
                        return Err(syn::Error::new_spanned(
                            &key,
                            format!("unknown key `{key}` in KeepDistance(...) (expected range)"),
                        ));
                    }
                    if content.peek(Token![,]) {
                        content.parse::<Token![,]>()?;
                    } else {
                        break;
                    }
                }
                Ok(Self::KeepDistance {
                    range: range.ok_or_else(|| {
                        syn::Error::new_spanned(&ident, "KeepDistance(...) requires `range = ...`")
                    })?,
                })
            }
            other => Err(syn::Error::new_spanned(
                &ident,
                format!(
                    "unknown movement `{other}` (expected Chase, KeepDistance(range = ...), \
                     Stationary, Hover)"
                ),
            )),
        }
    }

    fn to_tokens(&self) -> TokenStream2 {
        match self {
            Self::Chase => quote!(crate::placeables::CombatMovement::Chase),
            Self::KeepDistance { range } => {
                quote!(crate::placeables::CombatMovement::KeepDistance { range: #range })
            }
            Self::Stationary => quote!(crate::placeables::CombatMovement::Stationary),
            Self::Hover => quote!(crate::placeables::CombatMovement::Hover),
        }
    }
}

struct BossPhaseMacroDef {
    id: LitStr,
    hp_below: LitFloat,
    movement: CombatMovementDef,
}

impl Parse for BossPhaseMacroDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let content;
        parenthesized!(content in input);
        let mut id = None;
        let mut hp_below = None;
        let mut movement = None;
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "id" => id = Some(content.parse::<LitStr>()?),
                "hp_below" => hp_below = Some(parse_number_f32(&content)?),
                "movement" => movement = Some(CombatMovementDef::parse(&content)?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{other}` in phase (expected id, hp_below, movement)"),
                    ));
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(Self {
            id: id.ok_or_else(|| content.error("phase requires `id = \"...\"`"))?,
            hp_below: hp_below.ok_or_else(|| content.error("phase requires `hp_below = ...`"))?,
            movement: movement.ok_or_else(|| content.error("phase requires `movement = ...`"))?,
        })
    }
}

impl BossPhaseMacroDef {
    fn to_tokens(&self) -> TokenStream2 {
        let id = &self.id;
        let hp_below = &self.hp_below;
        let movement = self.movement.to_tokens();
        quote! {
            crate::placeables::BossPhaseDef {
                id: #id,
                hp_below: #hp_below,
                movement: #movement,
            }
        }
    }
}

struct EnemyDef {
    id: LitStr,
    kind: EnemyKind,
    name: LitStr,
    icon: Option<LitStr>,
    asset: LitStr,
    stats: EnemyStatsDef,
    aggro: LitFloat,
    leash_aggro: LitFloat,
    acquire: Ident,
    origin: Ident,
    threat: Ident,
    movement: CombatMovementDef,
    abilities: Vec<AbilityEntryDef>,
    arena: Option<LitFloat>,
    enrage_after: Option<LitFloat>,
    phases: Vec<BossPhaseMacroDef>,
    scale: Option<(LitFloat, LitFloat, LitFloat)>,
    tint: Option<(LitFloat, LitFloat, LitFloat)>,
    blocks_movement: bool,
    collision: CollisionSpec,
}

impl Parse for EnemyDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut id = None;
        let mut kind = None;
        let mut name = None;
        let mut icon = None;
        let mut asset = None;
        let mut stats = None;
        let mut aggro = None;
        let mut leash_aggro = None;
        let mut acquire = None;
        let mut origin = None;
        let mut threat = None;
        let mut movement = None;
        let mut abilities = None;
        let mut arena = None;
        let mut arena_span = None;
        let mut enrage_after = None;
        let mut enrage_span = None;
        let mut phases = Vec::new();
        let mut phases_span = None;
        let mut scale = None;
        let mut tint = None;
        let mut blocks_movement = false;
        let mut collision = CollisionSpec::None;

        while !input.is_empty() {
            let key: Ident = Ident::parse_any(input)?;
            let key_str = key.to_string();

            if key_str == "stats" {
                let content;
                parenthesized!(content in input);
                stats = Some(EnemyStatsDef::parse_from(&content)?);
            } else {
                input.parse::<Token![=]>()?;
                match key_str.as_str() {
                    "id" => id = Some(input.parse::<LitStr>()?),
                    "type" => {
                        let value: Ident = input.parse()?;
                        kind = Some(match value.to_string().as_str() {
                            "Normal" => EnemyKind::Normal,
                            "Boss" => EnemyKind::Boss,
                            other => {
                                return Err(syn::Error::new_spanned(
                                    &value,
                                    format!(
                                        "#[enemy(...)] `type` must be `Normal` or `Boss`, found `{other}`"
                                    ),
                                ));
                            }
                        });
                    }
                    "name" => name = Some(input.parse::<LitStr>()?),
                    "icon" => icon = Some(input.parse::<LitStr>()?),
                    "asset" => asset = Some(input.parse::<LitStr>()?),
                    "aggro" => aggro = Some(parse_number_f32(input)?),
                    "leash_aggro" => leash_aggro = Some(parse_number_f32(input)?),
                    "acquire" => acquire = Some(input.parse::<Ident>()?),
                    "origin" => origin = Some(input.parse::<Ident>()?),
                    "threat" => threat = Some(input.parse::<Ident>()?),
                    "movement" => movement = Some(CombatMovementDef::parse(input)?),
                    "abilities" => {
                        let content;
                        bracketed!(content in input);
                        let list: Punctuated<AbilityEntryDef, Token![,]> =
                            Punctuated::parse_terminated(&content)?;
                        abilities = Some(list.into_iter().collect::<Vec<_>>());
                    }
                    "arena" => {
                        arena_span = Some(key.span());
                        arena = Some(parse_number_f32(input)?);
                    }
                    "enrage_after" => {
                        enrage_span = Some(key.span());
                        enrage_after = Some(parse_number_f32(input)?);
                    }
                    "phases" => {
                        phases_span = Some(key.span());
                        let content;
                        bracketed!(content in input);
                        let list: Punctuated<BossPhaseMacroDef, Token![,]> =
                            Punctuated::parse_terminated(&content)?;
                        phases = list.into_iter().collect();
                    }
                    "scale" => scale = Some(parse_lit_triple(input)?),
                    "tint" => tint = Some(parse_lit_triple(input)?),
                    "blocks_movement" => blocks_movement = input.parse::<LitBool>()?.value(),
                    "collision" => collision = parse_collision_dsl(input)?,
                    other => {
                        return Err(syn::Error::new_spanned(
                            &key,
                            format!(
                                "unknown key `{other}` in #[enemy(...)] (expected id, type, name, \
                                 icon, asset, stats, aggro, leash_aggro, acquire, origin, threat, \
                                 movement, abilities, arena, enrage_after, phases, scale, tint, \
                                 blocks_movement, collision)"
                            ),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        let kind = kind.ok_or_else(|| {
            input.error("#[enemy(...)] requires `type = Normal` or `type = Boss`")
        })?;
        let abilities = abilities.ok_or_else(|| {
            input.error("#[enemy(...)] requires `abilities = [...]` with at least one ability")
        })?;
        if abilities.is_empty() {
            return Err(
                input.error("#[enemy(...)] requires `abilities = [...]` with at least one ability")
            );
        }

        match kind {
            EnemyKind::Normal => {
                if arena.is_some() || enrage_after.is_some() || !phases.is_empty() {
                    let span = arena_span
                        .or(enrage_span)
                        .or(phases_span)
                        .unwrap_or_else(proc_macro2::Span::call_site);
                    return Err(syn::Error::new(
                        span,
                        "#[enemy(...)] `type = Normal` cannot set `arena`, `phases`, or \
                         `enrage_after` (boss-only keys); use `type = Boss`",
                    ));
                }
            }
            EnemyKind::Boss => {
                if arena.is_none() {
                    return Err(input.error("#[enemy(...)] `type = Boss` requires `arena = ...`"));
                }
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| input.error("#[enemy(...)] requires `id = \"...\"`"))?,
            kind,
            name: name.ok_or_else(|| input.error("#[enemy(...)] requires `name = \"...\"`"))?,
            icon,
            asset: asset.ok_or_else(|| input.error("#[enemy(...)] requires `asset = \"...\"`"))?,
            stats: stats.ok_or_else(|| input.error("#[enemy(...)] requires `stats(...)`"))?,
            aggro: match (aggro, kind, &arena) {
                (Some(value), _, _) => value,
                (None, EnemyKind::Boss, Some(arena)) => arena.clone(),
                (None, _, _) => {
                    return Err(input.error(
                        "#[enemy(...)] requires `aggro = ...` (or `arena` when `type = Boss`)",
                    ));
                }
            },
            leash_aggro: match (leash_aggro, kind, &arena) {
                (Some(value), _, _) => value,
                (None, EnemyKind::Boss, Some(arena)) => arena.clone(),
                (None, _, _) => {
                    return Err(input.error(
                        "#[enemy(...)] requires `leash_aggro = ...` (or `arena` when `type = Boss`)",
                    ));
                }
            },
            acquire: acquire
                .unwrap_or_else(|| Ident::new("Proximity", proc_macro2::Span::call_site())),
            origin: origin.unwrap_or_else(|| {
                Ident::new(
                    match kind {
                        EnemyKind::Normal => "Body",
                        EnemyKind::Boss => "Spawn",
                    },
                    proc_macro2::Span::call_site(),
                )
            }),
            threat: threat.unwrap_or_else(|| {
                Ident::new(
                    match kind {
                        EnemyKind::Normal => "Nearest",
                        EnemyKind::Boss => "Table",
                    },
                    proc_macro2::Span::call_site(),
                )
            }),
            movement: movement.unwrap_or(CombatMovementDef::Chase),
            abilities,
            arena,
            enrage_after,
            phases,
            scale,
            tint,
            blocks_movement,
            collision,
        })
    }
}

impl EnemyDef {
    fn build_tokens(&self, original: &DeriveInput) -> syn::Result<TokenStream2> {
        let name = &original.ident;
        let id = &self.id;
        let display_name = &self.name;
        let icon = self
            .icon
            .clone()
            .unwrap_or_else(|| LitStr::new("❓", proc_macro2::Span::call_site()));
        let asset = &self.asset;
        let aggro = &self.aggro;
        let leash_aggro = &self.leash_aggro;
        let acquire = &self.acquire;
        let origin = &self.origin;
        let threat = &self.threat;
        let movement = self.movement.to_tokens();
        let ability_tokens: Vec<TokenStream2> = self
            .abilities
            .iter()
            .map(AbilityEntryDef::to_tokens)
            .collect();
        let stat_overrides = self.stats.override_tokens();
        let defaults_fn = match self.kind {
            EnemyKind::Normal => quote!(crate::stats::defaults::enemy_defaults),
            EnemyKind::Boss => quote!(crate::stats::defaults::boss_defaults),
        };
        let stats_init = if stat_overrides.is_empty() {
            quote! { let stats = #defaults_fn(); }
        } else {
            quote! {
                let mut stats = #defaults_fn();
                #(#stat_overrides)*
            }
        };
        let (scale_x, scale_y, scale_z) = match &self.scale {
            Some((x, y, z)) => (quote!(#x), quote!(#y), quote!(#z)),
            None => (quote!(1.0_f32), quote!(1.0_f32), quote!(1.0_f32)),
        };
        let tint_tokens = match &self.tint {
            Some((r, g, b)) => quote!(Some([#r, #g, #b])),
            None => quote!(None),
        };
        let blocks_movement = self.blocks_movement;
        let collision_tokens = build_collision_tokens(self.collision);
        let rank_tokens = match self.kind {
            EnemyKind::Normal => quote!(crate::placeables::EnemyRank::Normal),
            EnemyKind::Boss => quote!(crate::placeables::EnemyRank::Boss),
        };
        let arena_tokens = match &self.arena {
            Some(value) => quote!(Some(#value)),
            None => quote!(None),
        };
        let enrage_tokens = match &self.enrage_after {
            Some(value) => quote!(Some(#value)),
            None => quote!(None),
        };
        let phase_tokens: Vec<TokenStream2> = self
            .phases
            .iter()
            .map(BossPhaseMacroDef::to_tokens)
            .collect();
        let register_call = match self.kind {
            EnemyKind::Normal => quote! {
                registry.register_enemy(std::sync::Arc::new(#name))
            },
            EnemyKind::Boss => quote! {
                registry.register_boss(std::sync::Arc::new(#name))
            },
        };
        let boss_impl = match self.kind {
            EnemyKind::Boss => {
                let arena = self.arena.as_ref().expect("Boss requires arena");
                quote! {
                    impl crate::placeables::BossPlaceable for #name {
                        fn boss_config(&self) -> crate::placeables::BossConfig {
                            #stats_init
                            crate::placeables::BossConfig {
                                stats,
                                abilities: vec![#(#ability_tokens),*],
                                arena_radius: #arena,
                            }
                        }
                    }
                }
            }
            EnemyKind::Normal => quote! {},
        };

        Ok(quote! {
            #original

            impl #name {
                pub const ID: &'static str = #id;
            }

            impl crate::placeables::PlaceableDefinition for #name {
                fn id(&self) -> crate::placeables::KindId {
                    crate::placeables::KindId::new(Self::ID)
                }

                fn display_name(&self) -> &'static str {
                    #display_name
                }

                fn icon(&self) -> &'static str {
                    #icon
                }

                fn asset_hint(&self) -> crate::placeables::AssetHint {
                    crate::placeables::AssetHint::Scene(#asset)
                }

                fn defaults(&self) -> crate::placeables::PlaceableDefaults {
                    crate::placeables::PlaceableDefaults {
                        transform: crate::world::TransformData {
                            translation: [0.0_f32, 0.0_f32, 0.0_f32],
                            rotation_deg: [0.0_f32, 0.0_f32, 0.0_f32],
                            scale: [#scale_x, #scale_y, #scale_z],
                        },
                        tint: #tint_tokens,
                        collision: #collision_tokens,
                        blocks_movement: #blocks_movement,
                    }
                }
            }

            impl crate::placeables::EnemyPlaceable for #name {
                fn enemy_config(&self) -> crate::placeables::EnemyConfig {
                    #stats_init
                    crate::placeables::EnemyConfig {
                        stats,
                        aggro: #aggro,
                        leash_aggro: #leash_aggro,
                        abilities: vec![#(#ability_tokens),*],
                        acquire: crate::placeables::AcquirePolicy::#acquire,
                        origin: crate::placeables::AggroOrigin::#origin,
                        threat: crate::placeables::ThreatPolicy::#threat,
                        rank: #rank_tokens,
                        movement: #movement,
                        arena_radius: #arena_tokens,
                        enrage_after_seconds: #enrage_tokens,
                        phases: vec![#(#phase_tokens),*],
                    }
                }
            }

            #boss_impl

            pub fn register(registry: &mut crate::placeables::PlaceableRegistry) {
                #register_call
            }
        })
    }
}

fn parse_number_f32(input: ParseStream) -> syn::Result<LitFloat> {
    if input.peek(LitFloat) {
        input.parse()
    } else if input.peek(LitInt) {
        let int: LitInt = input.parse()?;
        Ok(LitFloat::new(
            &format!("{}.0", int.base10_digits()),
            int.span(),
        ))
    } else {
        Err(input.error("expected a number"))
    }
}

fn lit_f32(value: &str) -> LitFloat {
    LitFloat::new(value, proc_macro2::Span::call_site())
}

fn parse_lit_triple(input: ParseStream) -> syn::Result<(LitFloat, LitFloat, LitFloat)> {
    let content;
    parenthesized!(content in input);
    let x = parse_number_f32(&content)?;
    content.parse::<Token![,]>()?;
    let y = parse_number_f32(&content)?;
    content.parse::<Token![,]>()?;
    let z = parse_number_f32(&content)?;
    if content.peek(Token![,]) {
        content.parse::<Token![,]>()?;
    }
    if !content.is_empty() {
        return Err(content.error("expected exactly 3 numbers"));
    }
    Ok((x, y, z))
}

fn parse_collision_dsl(input: ParseStream) -> syn::Result<CollisionSpec> {
    let kind: Ident = Ident::parse_any(input)?;
    let content;
    parenthesized!(content in input);
    match kind.to_string().as_str() {
        "cylinder" => {
            let mut radius = None;
            let mut height = None;
            while !content.is_empty() {
                let key: Ident = content.parse()?;
                content.parse::<Token![=]>()?;
                let value = parse_number_f32(&content)?;
                match key.to_string().as_str() {
                    "radius" => radius = Some(value),
                    "height" => height = Some(value),
                    other => {
                        return Err(syn::Error::new_spanned(
                            &key,
                            format!(
                                "unknown key `{other}` in cylinder(...) (expected radius, height)"
                            ),
                        ));
                    }
                }
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
            let radius = radius
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "cylinder(...) requires `radius = ...`")
                })?
                .base10_parse::<f32>()?;
            let height = height
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "cylinder(...) requires `height = ...`")
                })?
                .base10_parse::<f32>()?;
            Ok(CollisionSpec::Cylinder { radius, height })
        }
        "sphere" => {
            let mut radius = None;
            while !content.is_empty() {
                let key: Ident = content.parse()?;
                content.parse::<Token![=]>()?;
                if key == "radius" {
                    radius = Some(parse_number_f32(&content)?);
                } else {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("unknown key `{key}` in sphere(...) (expected radius)"),
                    ));
                }
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
            let radius = radius
                .ok_or_else(|| {
                    syn::Error::new_spanned(&kind, "sphere(...) requires `radius = ...`")
                })?
                .base10_parse::<f32>()?;
            Ok(CollisionSpec::Sphere { radius })
        }
        "box" => {
            let values: Punctuated<LitFloat, Token![,]> = Punctuated::parse_terminated(&content)?;
            let values: Vec<LitFloat> = values.into_iter().collect();
            match values.as_slice() {
                [x, y, z] => Ok(CollisionSpec::Box {
                    half_x: x.base10_parse::<f32>()? * 0.5,
                    half_y: y.base10_parse::<f32>()? * 0.5,
                    half_z: z.base10_parse::<f32>()? * 0.5,
                }),
                _ => Err(syn::Error::new(
                    content.span(),
                    "box(...) requires (width, height, depth)",
                )),
            }
        }
        other => Err(syn::Error::new_spanned(
            &kind,
            format!("unknown collision `{other}` (expected cylinder, box, sphere)"),
        )),
    }
}

/// Attribute macro to define an interactive NPC.
///
/// Automatically implements:
/// - `PlaceableDefinition`
/// - `NpcPlaceable`
/// - `register()` function
///
/// # Example
///
/// ```rust,ignore
/// use bevymmo_shared::placeables::npc;
///
/// #[npc(
///     id = "npc_merchant",
///     name = "Merchant",
///     icon = "🧑‍💼",
///     asset = "models/npcs/merchant.glb",
///     tint = (0.6, 0.5, 0.9),
///     interaction = shop("shop_general"),
/// )]
/// pub struct Merchant;
/// ```
///
/// `asset` may be omitted or set to `placeholder` for a colored cuboid.
/// `interaction` is one of `shop("...")`, `market("...")`, `dialogue("...")`.
#[proc_macro_attribute]
pub fn npc(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let attrs = parse_npc_attrs(&attr.to_string());
    match expand_npc(&input, attrs) {
        Ok(tokens) => tokens.into(),
        Err(message) => syn::Error::new_spanned(&input.ident, message)
            .to_compile_error()
            .into(),
    }
}

enum NpcInteraction {
    Shop { inventory_id: String },
    Market { market_id: String },
    Dialogue { dialogue_tree_id: String },
    Craft { categories: Vec<String> },
}

struct NpcAttributes {
    props: PropsAttributes,
    interaction_raw: Option<String>,
}

fn parse_npc_attrs(input: &str) -> NpcAttributes {
    let mut attrs = NpcAttributes {
        props: parse_attrs(input),
        interaction_raw: None,
    };

    for pair in split_top_level_commas(input) {
        let pair = pair.trim();
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key.trim() == "interaction" {
            attrs.interaction_raw = Some(value.trim().to_string());
        }
    }

    attrs
}

fn parse_npc_interaction(value: &str) -> Result<NpcInteraction, String> {
    let value = value.trim();
    let Some((kind, rest)) = value.split_once('(') else {
        return Err(format!(
            "#[npc(...)] `interaction` must be shop(\"...\"), market(\"...\"), dialogue(\"...\"), or craft(Weapon, Tool), got `{value}`"
        ));
    };
    let kind = kind.trim();
    let inner = rest
        .trim()
        .strip_suffix(')')
        .unwrap_or(rest)
        .trim()
        .to_string();
    let id = clean_string(&inner).trim().to_string();
    if id.is_empty() {
        return Err(format!(
            "#[npc(...)] `{kind}(...)` requires a non-empty id string"
        ));
    }
    match kind {
        "shop" => Ok(NpcInteraction::Shop { inventory_id: id }),
        "market" => Ok(NpcInteraction::Market { market_id: id }),
        "dialogue" => Ok(NpcInteraction::Dialogue {
            dialogue_tree_id: id,
        }),
        "craft" => {
            const ALLOWED: &[&str] = &[
                "Weapon",
                "Armor",
                "Consumable",
                "Material",
                "Quest",
                "Accessory",
                "Tool",
            ];
            let categories: Vec<String> = id
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect();
            if categories.is_empty() {
                return Err(
                    "#[npc(...)] `craft(...)` requires at least one ItemCategory".to_string(),
                );
            }
            for category in &categories {
                if !ALLOWED.contains(&category.as_str()) {
                    return Err(format!(
                        "#[npc(...)] `craft(...)` expected an ItemCategory (Weapon | Armor | Consumable | Material | Quest | Accessory | Tool), got `{category}`"
                    ));
                }
            }
            Ok(NpcInteraction::Craft { categories })
        }
        other => Err(format!(
            "#[npc(...)] unknown interaction `{other}` (expected shop, market, dialogue, craft)"
        )),
    }
}

fn npc_asset_is_scene(asset: &str) -> bool {
    let asset = asset.trim();
    !asset.is_empty() && !asset.eq_ignore_ascii_case("placeholder")
}

fn expand_npc(input: &DeriveInput, attrs: NpcAttributes) -> Result<TokenStream2, String> {
    let name = &input.ident;
    let id = attrs
        .props
        .id
        .clone()
        .unwrap_or_else(|| name.to_string().to_lowercase());
    let display_name = attrs
        .props
        .display_name
        .clone()
        .unwrap_or_else(|| id.clone());
    let icon = attrs.props.icon.clone().unwrap_or_else(|| "👤".to_string());
    let interaction = match attrs.interaction_raw {
        Some(raw) => parse_npc_interaction(&raw)?,
        None => {
            return Err(
                "#[npc(...)] requires `interaction = shop(\"...\")` (or market/dialogue/craft)"
                    .to_string(),
            )
        }
    };

    let asset_hint = match attrs.props.asset.as_deref() {
        Some(path) if npc_asset_is_scene(path) => {
            quote!(crate::placeables::AssetHint::Scene(#path))
        }
        _ => quote!(crate::placeables::AssetHint::Placeholder),
    };

    let (scale_x, scale_y, scale_z) = attrs.props.scale.unwrap_or((1.0, 1.0, 1.0));
    let tint = attrs.props.tint.map(|(r, g, b)| quote!(Some([#r, #g, #b])));
    let tint_tokens = tint.unwrap_or_else(|| quote!(None));
    let blocks_movement = attrs.props.blocks_movement;
    let collision_tokens = build_collision_tokens(attrs.props.collision);

    let interaction_tokens = match &interaction {
        NpcInteraction::Shop { inventory_id } => quote! {
            crate::placeables::InteractionKind::Shop {
                inventory_id: #inventory_id.to_string(),
            }
        },
        NpcInteraction::Market { market_id } => quote! {
            crate::placeables::InteractionKind::Market {
                market_id: #market_id.to_string(),
            }
        },
        NpcInteraction::Dialogue { dialogue_tree_id } => quote! {
            crate::placeables::InteractionKind::Dialogue {
                dialogue_tree_id: #dialogue_tree_id.to_string(),
            }
        },
        NpcInteraction::Craft { categories } => {
            let category_idents: Vec<Ident> = categories
                .iter()
                .map(|category| Ident::new(category, proc_macro2::Span::call_site()))
                .collect();
            quote! {
                crate::placeables::InteractionKind::Craft {
                    categories: vec![#(crate::items::ItemCategory::#category_idents),*],
                }
            }
        }
    };

    Ok(quote! {
        #input

        impl crate::placeables::PlaceableDefinition for #name {
            fn id(&self) -> crate::placeables::KindId {
                crate::placeables::KindId::new(#id)
            }

            fn display_name(&self) -> &'static str {
                #display_name
            }

            fn icon(&self) -> &'static str {
                #icon
            }

            fn asset_hint(&self) -> crate::placeables::AssetHint {
                #asset_hint
            }

            fn defaults(&self) -> crate::placeables::PlaceableDefaults {
                crate::placeables::PlaceableDefaults {
                    transform: crate::world::TransformData {
                        translation: [0.0_f32, 0.0_f32, 0.0_f32],
                        rotation_deg: [0.0_f32, 0.0_f32, 0.0_f32],
                        scale: [#scale_x, #scale_y, #scale_z],
                    },
                    tint: #tint_tokens,
                    collision: #collision_tokens,
                    blocks_movement: #blocks_movement,
                }
            }
        }

        impl crate::placeables::NpcPlaceable for #name {
            fn interaction(&self) -> crate::placeables::InteractionKind {
                #interaction_tokens
            }
        }

        impl #name {
            pub const ID: &'static str = #id;

            pub fn register(registry: &mut crate::placeables::PlaceableRegistry) {
                registry.register_npc(std::sync::Arc::new(#name))
            }
        }

        pub fn register(registry: &mut crate::placeables::PlaceableRegistry) {
            #name::register(registry)
        }
    })
}
