//! Sistemi UI condivisi tra menu, settings e pause overlay.
//!
//! La visibilità di ciascuna schermata viene gestita con un cambio di
//! [`Display`] sul nodo root, non con respawn: lo spawn avviene una volta in
//! `Startup`.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use bevymmo_client::user_settings::{GameSettingsResource, KeyAction};

use crate::game_state::{
    validate_email, validate_password, validate_player_name, AuthIntent, AuthRequest,
    ConnectionFailure, ConnectionIntent, ConnectionRequest, EmailError, PasswordError,
    PauseOverlay, PlayerNameError, Screen, TypingFocus,
};
use crate::ui::button::{apply_button_image, UiButton, UiButtonAction, UiButtonImages};
use crate::ui::character_roster::SelectedRosterEntry;
use crate::ui::chat::ChatInput;
use crate::ui::inventory::components::SplitAmountField;
use crate::ui::login::{AuthPage, EmailInput, PasswordInput};
use crate::ui::main_menu::PlayerNameInput;
use crate::ui::settings::layout::SettingsTabButton;
use crate::ui::settings::widgets::{DropdownHeader, DropdownOption, KeyCapture};
use crate::ui::settings::{SettingsReturn, SettingsSession};
use crate::ui::text_input::{
    unfocus_all, TextInput, TextInputErrorText, TextInputImages, TextInputValueText,
};
use crate::ui::theme::UiTheme;

/// Keeps [`TypingFocus`] in sync with whichever text field actually has
/// focus this frame — chat, or one of the login/character-name `TextInput`
/// fields. Recomputed from scratch every frame rather than tracked
/// incrementally: several systems can change either field's `focused` flag
/// (click, Enter, Escape, sending a message, clicking the game world), and a
/// single source of truth here cannot drift out of sync with any of them.
///
/// Hidden fields are ignored: login/register/character-name inputs are spawned
/// at startup and only switched to `Display::None`, so a leftover
/// `focused: true` on a hidden node must not block gameplay keybinds. A
/// focused chat field that is actually on-screen still sets [`TypingFocus`].
///
/// `client`-crate gameplay systems (`send_combat_inputs`, `send_move_commands`)
/// read this — see [`bevymmo_client::app_state::TypingFocus`] for why it
/// lives there and not next to the components it mirrors.
pub(crate) fn sync_typing_focus(
    chat_inputs: Query<(Entity, &ChatInput)>,
    text_inputs: Query<(Entity, &TextInput)>,
    split_amounts: Query<(Entity, &SplitAmountField)>,
    nodes: Query<&Node>,
    parents: Query<&ChildOf>,
    mut typing: ResMut<TypingFocus>,
) {
    let focused = chat_inputs
        .iter()
        .any(|(entity, input)| input.focused && ui_tree_is_displayed(entity, &nodes, &parents))
        || text_inputs
            .iter()
            .any(|(entity, input)| input.focused && ui_tree_is_displayed(entity, &nodes, &parents))
        || split_amounts
            .iter()
            .any(|(entity, field)| field.focused && ui_tree_is_displayed(entity, &nodes, &parents));
    if typing.0 != focused {
        typing.0 = focused;
    }
}

/// Login/character-name [`TextInput`]s are not used on these screens; any
/// leftover focus would keep [`TypingFocus`] true and swallow I/K/Q/etc.
/// Chat uses [`ChatInput`], not [`TextInput`], so it is left alone.
pub(crate) fn unfocus_inputs_on_gameplay_screen(
    screen: Res<State<Screen>>,
    mut text_inputs: Query<&mut TextInput>,
    mut typing: ResMut<TypingFocus>,
) {
    if !matches!(*screen.get(), Screen::Connecting | Screen::InGame) {
        return;
    }
    let had_focus = text_inputs.iter().any(|input| input.focused);
    unfocus_all(&mut text_inputs);
    if had_focus || screen.is_changed() {
        typing.0 = false;
    }
}

/// Walks from `entity` to the UI root. A field whose own node or any ancestor
/// uses `Display::None` (hidden login/menu roots) is not capturing keyboard.
fn ui_tree_is_displayed(entity: Entity, nodes: &Query<&Node>, parents: &Query<&ChildOf>) -> bool {
    let mut current = Some(entity);
    while let Some(entity) = current {
        if let Ok(node) = nodes.get(entity) {
            if node.display == Display::None {
                return false;
            }
        }
        current = parents.get(entity).ok().map(|parent| parent.0);
    }
    true
}

fn error_message(err: PlayerNameError) -> String {
    match err {
        PlayerNameError::TooShort => "Name must be at least 3 characters.".to_string(),
        PlayerNameError::TooLong => "Name must be at most 16 characters.".to_string(),
    }
}

fn email_error_message(err: EmailError) -> String {
    match err {
        EmailError::MissingAt => "Email must contain '@'.".to_string(),
        EmailError::EmptyLocalOrDomain => "Email must have text before and after '@'.".to_string(),
        EmailError::DomainMissingDot => "Email domain must contain a '.'.".to_string(),
    }
}

fn password_error_message(err: PasswordError) -> String {
    match err {
        PasswordError::TooShort => "Password must be at least 8 characters.".to_string(),
    }
}

/// Dispatch delle azioni associate ai pulsanti UI.
///
/// Legge solo i pulsanti il cui [`Interaction`] è cambiato ed è `Pressed`.
pub fn update_button_actions(
    mut next_screen: ResMut<NextState<Screen>>,
    mut next_pause: ResMut<NextState<PauseOverlay>>,
    mut connection_request: ResMut<ConnectionRequest>,
    mut auth_page: ResMut<AuthPage>,
    screen: Res<State<Screen>>,
    pause: Option<Res<State<PauseOverlay>>>,
    mut settings_session: ResMut<SettingsSession>,
    selected: Option<Res<SelectedRosterEntry>>,
    buttons: Query<(&Interaction, &UiButton), Changed<Interaction>>,
    mut name_input: Query<&mut TextInput, With<PlayerNameInput>>,
) {
    let paused = pause.is_some_and(|pause| *pause.get() == PauseOverlay::On);
    let from_pause_menu = *screen.get() == Screen::InGame && paused;
    for (interaction, button) in buttons.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        match button.action {
            UiButtonAction::Play => {
                if let Some(SelectedRosterEntry::Existing(player_name)) = selected.as_deref() {
                    if let Ok(mut input) = name_input.single_mut() {
                        input.error = None;
                        input.focused = false;
                    }
                    next_screen.set(Screen::Connecting);
                    connection_request.0 = Some(ConnectionIntent::Connect {
                        player_name: player_name.clone(),
                    });
                    continue;
                }
                let Ok(mut input) = name_input.single_mut() else {
                    continue;
                };
                match validate_player_name(&input.value) {
                    Ok(name) => {
                        input.error = None;
                        input.focused = false;
                        next_screen.set(Screen::Connecting);
                        connection_request.0 =
                            Some(ConnectionIntent::Connect { player_name: name });
                    }
                    Err(err) => {
                        input.error = Some(error_message(err));
                    }
                }
            }
            // Handled by `update_auth_button_actions`, which needs the
            // email/password fields this system does not query.
            UiButtonAction::Login | UiButtonAction::Register => {}
            UiButtonAction::OpenRegister => {
                *auth_page = AuthPage::Register;
            }
            UiButtonAction::OpenLogin => {
                *auth_page = AuthPage::Login;
            }
            UiButtonAction::OpenSettings => {
                if from_pause_menu {
                    settings_session.open_from(SettingsReturn::Pause);
                } else {
                    settings_session.open_from(SettingsReturn::Menu);
                    next_screen.set(Screen::Settings);
                }
            }
            UiButtonAction::BackToMenu => {
                let from_pause =
                    settings_session.open && settings_session.return_to == SettingsReturn::Pause;
                settings_session.close();
                if !from_pause {
                    next_screen.set(Screen::MainMenu);
                }
            }
            UiButtonAction::ReturnToMainMenu => {
                settings_session.close();
                // Previously sent `Disconnect`, which the SpacetimeDB path
                // treats as a no-op — the character stayed marked online
                // server-side even though the screen had already left it.
                // `LeaveCharacter` is what `UiButtonAction::Logout` (pause
                // menu's "Leave Character") sends for the same reason.
                connection_request.0 = Some(ConnectionIntent::LeaveCharacter);
                next_screen.set(Screen::MainMenu);
            }
            UiButtonAction::Logout => {
                settings_session.close();
                // Pause menu's "Leave Character": returns to character
                // select, stays authenticated as the same account.
                connection_request.0 = Some(ConnectionIntent::LeaveCharacter);
                next_screen.set(Screen::MainMenu);
            }
            UiButtonAction::LogoutAccount => {
                // Character-select screen's "Logout": ends the account
                // session so a different one can sign in.
                connection_request.0 = Some(ConnectionIntent::LogoutAccount);
            }
            UiButtonAction::Resume => {
                next_pause.set(PauseOverlay::Off);
            }
            UiButtonAction::Exit => {
                // Goes through `stdb::plugin::begin_shutdown`/`finish_shutdown`
                // rather than writing `AppExit` directly, so the pending
                // disconnect actually reaches the socket before the process
                // dies. See `ConnectionIntent::Shutdown`.
                connection_request.0 = Some(ConnectionIntent::Shutdown);
            }
            // Handled by `settings::systems::reset_keybinds_on_button`.
            UiButtonAction::ResetKeybinds => {}
        }
    }
}

/// Dispatch delle azioni Login/Register del form di autenticazione.
///
/// Separato da [`update_button_actions`] perché legge due campi (email,
/// password) invece di uno solo, e nessun'altra azione ne ha bisogno.
pub fn update_auth_button_actions(
    mut auth_request: ResMut<AuthRequest>,
    buttons: Query<(&Interaction, &UiButton), Changed<Interaction>>,
    email_entities: Query<Entity, (With<EmailInput>, Without<PasswordInput>)>,
    password_entities: Query<Entity, (With<PasswordInput>, Without<EmailInput>)>,
    mut inputs: Query<&mut TextInput>,
) {
    for (interaction, button) in buttons.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let is_register = match button.action {
            UiButtonAction::Login => false,
            UiButtonAction::Register => true,
            _ => continue,
        };

        let Ok(email_entity) = email_entities.single() else {
            continue;
        };
        let Ok(password_entity) = password_entities.single() else {
            continue;
        };

        let email_result = {
            let Ok(mut email) = inputs.get_mut(email_entity) else {
                continue;
            };
            let result = validate_email(&email.value);
            email.error = result.clone().err().map(email_error_message);
            result
        };
        let password_result = {
            let Ok(mut password) = inputs.get_mut(password_entity) else {
                continue;
            };
            let result = validate_password(&password.value);
            password.error = result.clone().err().map(password_error_message);
            result
        };

        if let (Ok(normalized_email), Ok(())) = (email_result, password_result) {
            let password_value = inputs
                .get(password_entity)
                .map(|input| input.value.clone())
                .unwrap_or_default();
            auth_request.0 = Some(if is_register {
                AuthIntent::Register {
                    email: normalized_email,
                    password: password_value,
                }
            } else {
                AuthIntent::Login {
                    email: normalized_email,
                    password: password_value,
                }
            });
            unfocus_all(&mut inputs);
        }
    }
}

/// Aggiorna la texture in base allo stato di interazione.
///
/// Matches any node with [`UiButtonImages`] so Close / Equip / Respawn (which
/// are not [`UiButton`]) still swap the ornate bar on hover/press.
pub fn update_button_visuals(
    mut query: Query<
        (&Interaction, &mut ImageNode, &UiButtonImages),
        (
            Changed<Interaction>,
            Without<SettingsTabButton>,
            Without<DropdownHeader>,
            Without<DropdownOption>,
            Without<KeyCapture>,
        ),
    >,
) {
    for (interaction, mut image, button_images) in query.iter_mut() {
        apply_button_image(*interaction, &mut image, button_images);
    }
}

/// Focalizza il campo cliccato, sfocalizzando ogni altro campo aperto.
///
/// Più campi possono esistere insieme (es. email + password nel login): al
/// più uno è focalizzato alla volta, altrimenti la tastiera non saprebbe a
/// quale campo indirizzare gli eventi.
pub fn update_text_input_focus(
    clicked: Query<(Entity, &Interaction), (With<TextInput>, Changed<Interaction>)>,
    mut inputs: Query<(Entity, &mut TextInput)>,
) {
    let Some(clicked_entity) = clicked
        .iter()
        .find(|(_, interaction)| **interaction == Interaction::Pressed)
        .map(|(entity, _)| entity)
    else {
        return;
    };
    for (entity, mut input) in inputs.iter_mut() {
        input.focused = entity == clicked_entity;
    }
}

/// Gestione tastiera del campo di testo attualmente focalizzato, se c'è.
pub fn update_text_input_keyboard(
    mut events: MessageReader<KeyboardInput>,
    mut query: Query<&mut TextInput>,
) {
    let Some(mut input) = query.iter_mut().find(|input| input.focused) else {
        events.clear();
        return;
    };

    let len = input.value.chars().count();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Backspace => {
                input.value.pop();
            }
            Key::Enter | Key::Escape => {
                input.focused = false;
            }
            Key::Space if len < input.max_chars => {
                input.value.push(' ');
            }
            Key::Character(s) if len < input.max_chars => {
                // Un evento KeyboardInput può trasportare più di un carattere in
                // casi rari; prendiamo solo il primo stampabile ASCII.
                if let Some(ch) = s.chars().next().filter(|c| c.is_ascii_graphic()) {
                    input.value.push(ch);
                }
            }
            _ => {}
        }
    }
}

/// Riflette lo stato di ogni [`TextInput`] cambiato sui propri nodi testo
/// figli (valore/placeholder ed errore) e sulla texture idle/focused.
///
/// Scrive tramite gli entity id salvati su `TextInput` (`value_text`,
/// `error_text`), non tramite una ricerca globale: con più campi presenti
/// contemporaneamente non esiste "il" nodo valore, solo il nodo di *questo*
/// campo.
pub fn update_text_input_display(
    theme: Res<UiTheme>,
    query: Query<(Entity, &TextInput), Changed<TextInput>>,
    mut value_text: Query<(&mut Text, &mut TextColor), With<TextInputValueText>>,
    mut error_text: Query<&mut Text, (With<TextInputErrorText>, Without<TextInputValueText>)>,
    mut images: Query<(&mut ImageNode, &TextInputImages)>,
) {
    for (entity, input) in query.iter() {
        if let Ok((mut text, mut color)) = value_text.get_mut(input.value_text) {
            if input.value.is_empty() {
                text.0 = input.placeholder.clone();
                color.0 = theme.muted_text_color;
            } else {
                text.0 = if input.obscured {
                    "•".repeat(input.value.chars().count())
                } else {
                    input.value.clone()
                };
                color.0 = theme.text_color;
            }
        }

        if let Ok(mut text) = error_text.get_mut(input.error_text) {
            text.0 = input.error.clone().unwrap_or_default();
        }

        if let Ok((mut image, input_images)) = images.get_mut(entity) {
            image.image = if input.focused {
                input_images.focused.clone()
            } else {
                input_images.idle.clone()
            };
        }
    }
}

/// Keeps the visible slice of a single-line field on the caret (the end,
/// since this widget only appends/pops). Long values scroll instead of
/// growing out of the bar.
pub fn scroll_text_input_to_caret(
    inputs: Query<&TextInput>,
    computed: Query<&ComputedNode>,
    mut scrolls: Query<&mut ScrollPosition>,
) {
    for input in inputs.iter() {
        if input.viewport == Entity::PLACEHOLDER {
            continue;
        }
        let Ok(view) = computed.get(input.viewport) else {
            continue;
        };
        let Ok(text) = computed.get(input.value_text) else {
            continue;
        };
        let Ok(mut scroll) = scrolls.get_mut(input.viewport) else {
            continue;
        };
        let view_w = view.size().x * view.inverse_scale_factor();
        let text_w = text.size().x * text.inverse_scale_factor();
        let max_scroll = (text_w - view_w).max(0.0);
        scroll.0.x = if input.focused { max_scroll } else { 0.0 };
    }
}

/// Aggiorna il testo che mostra l'eventuale errore di connessione nel menu
/// principale, leggendolo da [`ConnectionFailure`]. Non tocca
/// [`TextInput::error`] (validazione nome): i due canali sono indipendenti.
pub fn update_connection_failure(
    failure: Res<ConnectionFailure>,
    mut query: Query<&mut Text, With<crate::ui::main_menu::MainMenuConnectionFailure>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };
    text.0 = failure.0.clone().unwrap_or_default();
}

/// Mostra/nasconde il pause overlay with the configured `TogglePause` key,
/// only while [`Screen::InGame`].
///
/// Non tocca `Time`, `FixedUpdate` o la rete. `State<PauseOverlay>` is absent
/// in menus, so this must not require that resource.
pub fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<GameSettingsResource>,
    screen: Res<State<Screen>>,
    pause: Option<Res<State<PauseOverlay>>>,
    mut next_pause: ResMut<NextState<PauseOverlay>>,
) {
    if !settings.just_pressed(KeyAction::TogglePause, &keys) {
        return;
    }
    if *screen.get() != Screen::InGame {
        return;
    }
    let Some(pause) = pause else {
        return;
    };
    match *pause.get() {
        PauseOverlay::Off => next_pause.set(PauseOverlay::On),
        PauseOverlay::On => next_pause.set(PauseOverlay::Off),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::init_screen_states;
    use bevymmo_client::user_settings::{GameSettings, KeyBinding, KeyModifiers};

    fn current_screen(app: &App) -> Screen {
        *app.world().resource::<State<Screen>>().get()
    }

    fn current_pause(app: &App) -> Option<PauseOverlay> {
        app.world()
            .get_resource::<State<PauseOverlay>>()
            .map(|pause| *pause.get())
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        init_screen_states(&mut app);
        app.insert_resource(GameSettingsResource(GameSettings::default()));
        app.add_systems(Update, toggle_pause);
        app.insert_state(Screen::InGame);
        // Sub-state exists only after the InGame enter transition.
        app.update();
        app
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
    }

    #[test]
    fn pause_uses_default_escape_binding() {
        let mut app = test_app();

        press(&mut app, KeyCode::KeyP);
        app.update();
        assert_eq!(current_screen(&app), Screen::InGame);
        assert_eq!(current_pause(&app), Some(PauseOverlay::Off));

        press(&mut app, KeyCode::Escape);
        app.update();
        // NextState is applied in StateTransition, which runs before Update.
        app.update();
        assert_eq!(current_screen(&app), Screen::InGame);
        assert_eq!(current_pause(&app), Some(PauseOverlay::On));
    }

    #[test]
    fn pause_respects_custom_binding() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<GameSettingsResource>()
            .0
            .keybinds
            .bindings
            .insert(
                KeyAction::TogglePause,
                KeyBinding {
                    key: KeyCode::KeyP,
                    modifiers: KeyModifiers::default(),
                },
            );

        press(&mut app, KeyCode::Escape);
        app.update();
        assert_eq!(current_screen(&app), Screen::InGame);
        assert_eq!(current_pause(&app), Some(PauseOverlay::Off));

        press(&mut app, KeyCode::KeyP);
        app.update();
        // NextState is applied in StateTransition, which runs before Update.
        app.update();
        assert_eq!(current_screen(&app), Screen::InGame);
        assert_eq!(current_pause(&app), Some(PauseOverlay::On));
    }

    #[test]
    fn logout_button_leaves_character_and_transitions_to_main_menu() {
        use crate::ui::button::{UiButton, UiButtonAction};

        let mut app = App::new();
        init_screen_states(&mut app);
        app.init_resource::<ConnectionRequest>();
        app.init_resource::<AuthPage>();
        app.init_resource::<crate::ui::settings::SettingsSession>();
        app.insert_resource(GameSettingsResource(GameSettings::default()));

        // Spawn a button with Logout action
        let button_entity = app
            .world_mut()
            .spawn((
                UiButton {
                    action: UiButtonAction::Logout,
                },
                Interaction::Pressed,
            ))
            .id();

        app.add_systems(Update, update_button_actions);
        app.update();
        // NextState is applied in StateTransition, which runs before Update.
        app.update();

        // Verify screen transitioned to MainMenu
        assert_eq!(current_screen(&app), Screen::MainMenu);

        // Verify connection request was set to leave the character (not to
        // log out of the account — see `UiButtonAction::LogoutAccount` for that).
        let connection_request = app.world().resource::<ConnectionRequest>();
        assert!(connection_request.0.is_some());
        assert!(matches!(
            connection_request.0.as_ref().unwrap(),
            ConnectionIntent::LeaveCharacter
        ));

        // Cleanup
        app.world_mut().despawn(button_entity);
    }

    fn spawn_test_text_input(world: &mut World, value: &str, focused: bool) -> Entity {
        let value_text = world.spawn_empty().id();
        let error_text = world.spawn_empty().id();
        world
            .spawn(TextInput {
                value: value.to_string(),
                focused,
                error: None,
                placeholder: String::new(),
                max_chars: 64,
                obscured: false,
                value_text,
                error_text,
                viewport: Entity::PLACEHOLDER,
            })
            .id()
    }

    #[derive(Resource, Default)]
    struct GameplayRan(bool);

    fn gameplay_if_not_typing(mut ran: ResMut<GameplayRan>) {
        ran.0 = true;
    }

    fn play_test_app() -> App {
        let mut app = App::new();
        init_screen_states(&mut app);
        app.init_resource::<ConnectionRequest>();
        app.init_resource::<AuthPage>();
        app.init_resource::<crate::ui::settings::SettingsSession>();
        app.init_resource::<SelectedRosterEntry>();
        app.add_systems(Update, update_button_actions);
        app
    }

    fn press_play(app: &mut App) {
        app.world_mut().spawn((
            UiButton {
                action: UiButtonAction::Play,
            },
            Interaction::Pressed,
        ));
    }

    fn spawn_player_name(app: &mut App, value: &str) -> Entity {
        let entity = spawn_test_text_input(app.world_mut(), value, false);
        app.world_mut().entity_mut(entity).insert(PlayerNameInput);
        entity
    }

    #[test]
    fn play_with_selected_roster_character_skips_name_validation() {
        let mut app = play_test_app();
        *app.world_mut().resource_mut::<SelectedRosterEntry>() =
            SelectedRosterEntry::Existing("Al".into());
        let name = spawn_player_name(&mut app, "x");
        press_play(&mut app);
        app.update();
        // NextState is applied in StateTransition, which runs before Update.
        app.update();

        assert_eq!(current_screen(&app), Screen::Connecting);
        assert!(matches!(
            app.world().resource::<ConnectionRequest>().0,
            Some(ConnectionIntent::Connect { ref player_name }) if player_name == "Al"
        ));
        assert!(
            app.world()
                .entity(name)
                .get::<TextInput>()
                .is_some_and(|input| input.error.is_none()),
            "existing roster names must not run the 3-char create-name check"
        );
    }

    #[test]
    fn opening_settings_from_pause_keeps_the_world_loaded() {
        use crate::ui::button::{UiButton, UiButtonAction};
        use crate::ui::settings::{SettingsReturn, SettingsSession};

        let mut app = App::new();
        init_screen_states(&mut app);
        app.init_resource::<ConnectionRequest>();
        app.init_resource::<AuthPage>();
        app.init_resource::<SettingsSession>();
        app.insert_resource(GameSettingsResource(GameSettings::default()));
        app.insert_state(Screen::InGame);
        app.update();
        app.world_mut()
            .resource_mut::<NextState<PauseOverlay>>()
            .set(PauseOverlay::On);
        app.update();

        app.world_mut().spawn((
            UiButton {
                action: UiButtonAction::OpenSettings,
            },
            Interaction::Pressed,
        ));
        app.add_systems(Update, update_button_actions);
        app.update();

        assert_eq!(current_screen(&app), Screen::InGame);
        assert_eq!(current_pause(&app), Some(PauseOverlay::On));
        let session = app.world().resource::<SettingsSession>();
        assert!(session.open);
        assert_eq!(session.return_to, SettingsReturn::Pause);
    }

    #[test]
    fn back_from_menu_settings_returns_to_main_menu() {
        use crate::ui::button::{UiButton, UiButtonAction};
        use crate::ui::settings::{SettingsReturn, SettingsSession};

        let mut app = App::new();
        init_screen_states(&mut app);
        app.init_resource::<ConnectionRequest>();
        app.init_resource::<AuthPage>();
        app.init_resource::<SettingsSession>();
        app.insert_resource(GameSettingsResource(GameSettings::default()));
        app.insert_state(Screen::Settings);
        app.world_mut()
            .resource_mut::<SettingsSession>()
            .open_from(SettingsReturn::Menu);

        app.world_mut().spawn((
            UiButton {
                action: UiButtonAction::BackToMenu,
            },
            Interaction::Pressed,
        ));
        app.add_systems(Update, update_button_actions);
        app.update();
        app.update();

        assert_eq!(current_screen(&app), Screen::MainMenu);
        assert!(!app.world().resource::<SettingsSession>().open);
    }

    #[test]
    fn play_with_create_selected_validates_the_name_field() {
        let mut app = play_test_app();
        *app.world_mut().resource_mut::<SelectedRosterEntry>() = SelectedRosterEntry::Create;
        let name = spawn_player_name(&mut app, "ab");
        press_play(&mut app);
        app.update();

        assert_eq!(current_screen(&app), Screen::MainMenu);
        assert!(app.world().resource::<ConnectionRequest>().0.is_none());
        assert!(app
            .world()
            .entity(name)
            .get::<TextInput>()
            .is_some_and(|input| input.error.is_some()));
    }

    #[test]
    fn play_with_nothing_selected_uses_the_name_field() {
        let mut app = play_test_app();
        spawn_player_name(&mut app, "Ada");
        press_play(&mut app);
        app.update();
        // NextState is applied in StateTransition, which runs before Update.
        app.update();

        assert_eq!(current_screen(&app), Screen::Connecting);
        assert!(matches!(
            app.world().resource::<ConnectionRequest>().0,
            Some(ConnectionIntent::Connect { ref player_name }) if player_name == "Ada"
        ));
    }

    #[test]
    fn hidden_focused_password_does_not_block_gameplay_keys() {
        let mut app = App::new();
        app.init_resource::<TypingFocus>();
        app.init_resource::<GameplayRan>();
        init_screen_states(&mut app);
        app.insert_state(Screen::InGame);
        app.world_mut().resource_mut::<TypingFocus>().0 = true;

        let hidden_root = app
            .world_mut()
            .spawn(Node {
                display: Display::None,
                ..default()
            })
            .id();
        let password = spawn_test_text_input(app.world_mut(), "password1", true);
        app.world_mut().entity_mut(password).insert((
            PasswordInput,
            Node::default(),
            ChildOf(hidden_root),
        ));

        app.add_systems(
            Update,
            (
                sync_typing_focus,
                gameplay_if_not_typing.run_if(crate::game_state::not_typing),
            )
                .chain(),
        );
        app.update();

        assert!(
            !app.world().resource::<TypingFocus>().0,
            "hidden login fields must not hold TypingFocus in-game"
        );
        assert!(
            app.world().resource::<GameplayRan>().0,
            "not_typing must be true so gameplay keys run"
        );
        assert!(
            app.world()
                .entity(password)
                .get::<TextInput>()
                .is_some_and(|input| input.focused),
            "sync must ignore hidden focus without requiring the flag to already be cleared"
        );
    }

    #[test]
    fn visible_focused_login_field_keeps_typing_focus() {
        let mut app = App::new();
        app.init_resource::<TypingFocus>();
        init_screen_states(&mut app);

        let root = app
            .world_mut()
            .spawn(Node {
                display: Display::Flex,
                ..default()
            })
            .id();
        let email = spawn_test_text_input(app.world_mut(), "user@example.com", true);
        app.world_mut()
            .entity_mut(email)
            .insert((EmailInput, Node::default(), ChildOf(root)));

        app.add_systems(Update, sync_typing_focus);
        app.update();

        assert!(
            app.world().resource::<TypingFocus>().0,
            "visible login fields must still capture keys"
        );
    }

    #[test]
    fn focused_split_amount_field_holds_typing_focus() {
        use crate::ui::inventory::components::SplitAmountField;

        let mut app = App::new();
        app.init_resource::<TypingFocus>();
        init_screen_states(&mut app);
        app.insert_state(Screen::InGame);

        app.world_mut().spawn((
            Node::default(),
            SplitAmountField {
                value: "7".into(),
                focused: true,
                quantity: 50,
            },
        ));
        app.add_systems(Update, sync_typing_focus);
        app.update();

        assert!(
            app.world().resource::<TypingFocus>().0,
            "typing a split amount must not fire I/WASD"
        );
    }

    #[test]
    fn successful_login_unfocuses_all_text_inputs() {
        let mut app = App::new();
        app.init_resource::<AuthRequest>();
        app.add_systems(Update, update_auth_button_actions);

        let email = spawn_test_text_input(app.world_mut(), "user@example.com", true);
        app.world_mut().entity_mut(email).insert(EmailInput);
        let password = spawn_test_text_input(app.world_mut(), "password1", true);
        app.world_mut().entity_mut(password).insert(PasswordInput);
        let extra = spawn_test_text_input(app.world_mut(), "Ada", true);

        app.world_mut().spawn((
            UiButton {
                action: UiButtonAction::Login,
            },
            Interaction::Pressed,
        ));

        app.update();

        for entity in [email, password, extra] {
            let input = app
                .world()
                .entity(entity)
                .get::<TextInput>()
                .expect("text input");
            assert!(
                !input.focused,
                "successful login must unfocus every TextInput"
            );
        }
        assert!(matches!(
            app.world().resource::<AuthRequest>().0,
            Some(AuthIntent::Login { .. })
        ));
    }

    #[test]
    fn entering_ingame_unfocuses_text_inputs_and_clears_typing_focus() {
        let mut app = App::new();
        app.init_resource::<TypingFocus>();
        init_screen_states(&mut app);
        app.world_mut().resource_mut::<TypingFocus>().0 = true;

        let input = spawn_test_text_input(app.world_mut(), "password1", true);
        app.add_systems(Update, unfocus_inputs_on_gameplay_screen);

        app.update();
        assert!(
            app.world()
                .entity(input)
                .get::<TextInput>()
                .is_some_and(|input| input.focused),
            "MainMenu must leave login/character-name fields focusable"
        );

        app.insert_state(Screen::Connecting);
        app.update();

        assert!(app
            .world()
            .entity(input)
            .get::<TextInput>()
            .is_some_and(|input| !input.focused));
        assert!(!app.world().resource::<TypingFocus>().0);

        app.world_mut()
            .entity_mut(input)
            .get_mut::<TextInput>()
            .expect("text input")
            .focused = true;
        app.world_mut().resource_mut::<TypingFocus>().0 = true;
        app.insert_state(Screen::InGame);
        app.update();

        assert!(app
            .world()
            .entity(input)
            .get::<TextInput>()
            .is_some_and(|input| !input.focused));
        assert!(!app.world().resource::<TypingFocus>().0);
    }
}
