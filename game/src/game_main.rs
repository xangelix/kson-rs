use std::{
    rc::Rc,
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};

use egui_glow::{winit, EguiGlow};
use femtovg::Paint;
use game_loop::{
    winit::{dpi::PhysicalPosition, event, monitor::VideoMode, window::Window},
    GameLoop, Time,
};
use generational_arena::{Arena, Index};

use kson::Chart;
use log::*;
use puffin::{profile_function, profile_scope};

use serde_json::json;
use td::FrameOutput;
use tealr::mlu::mlua::Lua;
use three_d::FrameInput;

use femtovg as vg;
use three_d as td;
use vg::{renderer::OpenGl, Canvas};

use tealr::mlu::mlua::LuaSerdeExt;

use crate::{
    button_codes::{LaserState, UscInputEvent},
    config::GameConfig,
    default_game_dir,
    game::HitRating,
    game_data::{ExportGame, GameData, LuaPath},
    input_state::InputState,
    lua_http::{ExportLuaHttp, LuaHttp},
    main_menu::MainMenuButton,
    scene,
    settings_screen::SettingsScreen,
    songselect,
    transition::Transition,
    util::lua_address,
    vg_ui::{ExportVgfx, Vgfx},
    RuscMixer, Scenes, FRAME_ACC_SIZE,
};

type SceneLoader = dyn FnOnce() -> (Chart, Box<dyn rodio::Source<Item = f32> + Send>) + Send;

pub enum ControlMessage {
    None,
    MainMenu(MainMenuButton),
    Song {
        song: Arc<songselect::Song>,
        diff: usize,
        loader: Box<SceneLoader>,
    },
    TransitionComplete(Box<dyn scene::Scene>),
    Result {
        song: Arc<songselect::Song>,
        diff_idx: usize,
        score: u32,
        gauge: f32,
        hit_ratings: Vec<HitRating>,
    },
}

impl Default for ControlMessage {
    fn default() -> Self {
        Self::None
    }
}

pub struct GameMain {
    lua_arena: Rc<RwLock<Arena<Rc<Lua>>>>,
    scenes: Scenes,
    control_tx: Sender<ControlMessage>,
    control_rx: Receiver<ControlMessage>,
    knob_state: LaserState,
    frame_times: [f64; 16],
    frame_time_index: usize,
    fps_paint: Paint,
    transition_lua_idx: Index,
    transition_song_lua_idx: Index,
    game_data: Arc<Mutex<GameData>>,
    vgfx: Arc<Mutex<Vgfx>>,
    canvas: Arc<Mutex<Canvas<OpenGl>>>,
    frame_count: u32,
    gui: EguiGlow,
    show_debug_ui: bool,
    mousex: f64,
    mousey: f64,
    input_state: InputState,
    mixer: RuscMixer,
}

impl GameMain {
    pub fn new(
        lua_arena: Rc<RwLock<Arena<Rc<Lua>>>>,
        scenes: Scenes,
        control_tx: Sender<ControlMessage>,
        control_rx: Receiver<ControlMessage>,
        knob_state: LaserState,
        frame_times: [f64; 16],
        frame_time_index: usize,
        fps_paint: Paint,
        transition_lua_idx: Index,
        transition_song_lua_idx: Index,
        game_data: Arc<Mutex<GameData>>,
        vgfx: Arc<Mutex<Vgfx>>,
        canvas: Arc<Mutex<Canvas<OpenGl>>>,
        frame_count: u32,
        gui: EguiGlow,
        show_debug_ui: bool,
        mousex: f64,
        mousey: f64,
        input_state: InputState,
        mixer: RuscMixer,
    ) -> Self {
        Self {
            lua_arena,
            scenes,
            control_tx,
            control_rx,
            knob_state,
            frame_times,
            frame_time_index,
            fps_paint,
            transition_lua_idx,
            transition_song_lua_idx,
            game_data,
            vgfx,
            canvas,
            frame_count,
            gui,
            show_debug_ui,
            mousex,
            mousey,
            input_state,
            mixer,
        }
    }

    const KEYBOARD_LASER_SENS: f32 = 1.0 / 240.0;
    pub fn update(&mut self) {
        let should_profile = GameConfig::get().args.profiling;
        if puffin::are_scopes_on() != should_profile {
            puffin::set_scopes_on(should_profile);
        }

        if GameConfig::get().keyboard_knobs {
            let mut ls = LaserState::default();
            for l in [kson::Side::Left, kson::Side::Right] {
                for d in [kson::Side::Left, kson::Side::Right] {
                    if self
                        .input_state
                        .is_button_held(crate::button_codes::UscButton::Laser(l, d))
                        .is_some()
                    {
                        ls.update(
                            l,
                            match d {
                                kson::Side::Left => -Self::KEYBOARD_LASER_SENS,
                                kson::Side::Right => Self::KEYBOARD_LASER_SENS,
                            },
                        )
                    }
                }
            }

            self.scenes.for_each_active_mut(|x| {
                x.on_event(&event::Event::UserEvent(UscInputEvent::Laser(ls)))
            });
        }
    }
    pub fn render(
        &mut self,
        frame_input: FrameInput<()>,
        window: &game_loop::winit::window::Window,
    ) -> FrameOutput {
        let GameMain {
            lua_arena,
            scenes,
            control_tx,
            control_rx,
            knob_state,
            frame_times,
            fps_paint,
            transition_lua_idx,
            transition_song_lua_idx,
            canvas,
            frame_count,
            game_data,
            vgfx,
            show_debug_ui,
            gui,
            frame_time_index,
            mousex,
            mousey,
            input_state: _,
            mixer,
        } = self;

        knob_state.zero_deltas();
        puffin::profile_scope!("Frame");
        puffin::GlobalProfiler::lock().new_frame();

        for (_idx, lua) in lua_arena.read().unwrap().iter() {
            lua.set_app_data(frame_input.clone());
        }
        let lua_frame_input = frame_input.clone();
        let lua_mixer = mixer.clone();
        let load_lua = |game_data: Arc<Mutex<GameData>>,
                        vgfx: Arc<Mutex<Vgfx>>,
                        arena: Rc<RwLock<Arena<Rc<Lua>>>>| {
            let lua_frame_input = lua_frame_input.clone();
            let lua_mixer = lua_mixer.clone();
            Rc::new(move |lua: Rc<Lua>, script_path| {
                //Set path for 'require' (https://stackoverflow.com/questions/4125971/setting-the-global-lua-path-variable-from-c-c?lq=1)
                let skin = &GameConfig::get().skin;
                let mut real_script_path = default_game_dir();
                real_script_path.push("skins");
                real_script_path.push(skin);

                tealr::mlu::set_global_env(ExportVgfx, &lua)?;
                tealr::mlu::set_global_env(ExportGame, &lua)?;
                tealr::mlu::set_global_env(LuaPath, &lua)?;
                tealr::mlu::set_global_env(ExportLuaHttp, &lua)?;
                lua.globals()
                    .set(
                        "IRData",
                        lua.to_value(&json!({
                            "Active": false
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                let idx = arena
                    .write()
                    .expect("Could not get lock to lua arena")
                    .insert(lua.clone());

                {
                    vgfx.lock().unwrap().init_asset_scope(lua_address(&lua))
                }

                {
                    lua.set_app_data(vgfx.clone());
                    lua.set_app_data(game_data.clone());
                    lua.set_app_data(lua_frame_input.clone());
                    lua.set_app_data(lua_mixer.clone());
                    lua.set_app_data(LuaHttp::default());
                    //lua.gc_stop();
                }

                {
                    let package: tealr::mlu::mlua::Table = lua.globals().get("package").unwrap();
                    let package_path = format!(
                        "{0}/scripts/?.lua;{0}/scripts/?",
                        real_script_path.as_os_str().to_string_lossy()
                    );
                    package.set("path", package_path).unwrap();

                    lua.globals().set("package", package).unwrap();
                }

                real_script_path.push("scripts");

                real_script_path.push("common.lua");
                if real_script_path.exists() {
                    info!("Loading: {:?}", &real_script_path);
                    let test_code = std::fs::read_to_string(&real_script_path)?;
                    lua.load(&test_code).set_name("common.lua")?.eval::<()>()?;
                }

                real_script_path.pop();

                real_script_path.push(script_path);
                info!("Loading: {:?}", &real_script_path);
                let test_code = std::fs::read_to_string(real_script_path)?;
                {
                    profile_scope!("evaluate lua file");
                    lua.load(&test_code).set_name(script_path)?.eval::<()>()?;
                }
                Ok(idx)
            })
        };

        if frame_input.first_frame {
            frame_input
                .screen()
                .clear(td::ClearState::color(0.0, 0.0, 0.0, 1.0));
            let mut canvas = canvas.lock().unwrap();
            canvas.reset();
            canvas.set_size(frame_input.viewport.width, frame_input.viewport.height, 1.0);
            canvas.fill_text(
                10.0,
                10.0,
                "Loading...",
                &vg::Paint::color(vg::Color::white())
                    .with_font_size(32.0)
                    .with_text_baseline(vg::Baseline::Top),
            );
            canvas.flush();
            *frame_count += 1;

            return FrameOutput {
                swap_buffers: true,
                wait_next_event: false,
                ..Default::default()
            };
        }
        if *frame_count == 1 {
            let transition_lua = Rc::new(Lua::new());
            let loader_fn = load_lua(game_data.clone(), vgfx.clone(), lua_arena.clone());
            *transition_lua_idx = loader_fn(transition_lua, "transition.lua").unwrap();

            let transition_song_lua = Rc::new(Lua::new());
            *transition_song_lua_idx =
                loader_fn(transition_song_lua, "songtransition.lua").unwrap();
            *frame_count += 1;
        }

        //Initialize loaded scenes
        scenes.tick(
            frame_input.elapsed_time,
            *knob_state,
            load_lua(game_data.clone(), vgfx.clone(), lua_arena.clone()),
            control_tx.clone(),
        );

        while let Ok(control_msg) = control_rx.try_recv() {
            match control_msg {
                ControlMessage::None => {}
                ControlMessage::MainMenu(b) => match b {
                    MainMenuButton::Start => {
                        scenes.suspend_top();

                        if let Ok(arena) = lua_arena.read() {
                            let transition_lua = arena.get(*transition_lua_idx).unwrap().clone();
                            scenes.transition = Some(Transition::new(
                                transition_lua,
                                ControlMessage::MainMenu(MainMenuButton::Start),
                                control_tx.clone(),
                                frame_input.context.clone(),
                                vgfx.clone(),
                                frame_input.viewport,
                                self.input_state.clone(),
                                game_data.clone(),
                            ))
                        }
                    }
                    MainMenuButton::Downloads => {}
                    MainMenuButton::Exit => {
                        scenes.clear();
                    }
                    MainMenuButton::Options => scenes.loaded.push(Box::new(SettingsScreen::new())),
                    _ => {}
                },
                ControlMessage::Song { diff, loader, song } => {
                    if let Ok(arena) = lua_arena.read() {
                        let transition_lua = arena.get(*transition_song_lua_idx).unwrap().clone();
                        scenes.transition = Some(Transition::new(
                            transition_lua,
                            ControlMessage::Song { diff, loader, song },
                            control_tx.clone(),
                            frame_input.context.clone(),
                            vgfx.clone(),
                            frame_input.viewport,
                            self.input_state.clone(),
                            game_data.clone(),
                        ))
                    }
                }
                ControlMessage::TransitionComplete(scene_data) => scenes.loaded.push(scene_data),
                ControlMessage::Result {
                    song,
                    diff_idx,
                    score,
                    gauge,
                    hit_ratings,
                } => {
                    if let Ok(arena) = lua_arena.read() {
                        let transition_lua = arena.get(*transition_lua_idx).unwrap().clone();
                        scenes.transition = Some(Transition::new(
                            transition_lua,
                            ControlMessage::Result {
                                song,
                                diff_idx,
                                score,
                                gauge,
                                hit_ratings,
                            },
                            control_tx.clone(),
                            frame_input.context.clone(),
                            vgfx.clone(),
                            frame_input.viewport,
                            self.input_state.clone(),
                            game_data.clone(),
                        ))
                    }
                }
            }
        }

        frame_times[*frame_time_index] = frame_input.elapsed_time;
        *frame_time_index = (*frame_time_index + 1) % FRAME_ACC_SIZE;
        let fps = 1000_f64 / (frame_times.iter().sum::<f64>() / FRAME_ACC_SIZE as f64);

        Self::update_game_data_and_clear(game_data, *mousex, *mousey, &frame_input, *knob_state);

        Self::reset_viewport_size(vgfx.clone(), &frame_input);

        scenes.render(frame_input.clone(), vgfx);
        Self::render_overlays(vgfx, &frame_input, fps, fps_paint);

        gui.run(window, |ctx| {
            scenes.render_egui(ctx);

            if *show_debug_ui {
                Self::debug_ui(ctx, scenes);
            }
        });
        gui.paint(window);

        Self::run_lua_gc(
            lua_arena,
            &mut vgfx.lock().unwrap(),
            *transition_lua_idx,
            *transition_song_lua_idx,
        );

        if let Ok(mut a) = game_data.lock() {
            a.profile_stack.clear()
        }

        let exit = scenes.is_empty();
        if exit {
            GameConfig::get().save()
        }

        FrameOutput {
            exit,
            swap_buffers: true,
            wait_next_event: false,
        }
    }
    pub fn handle(
        &mut self,
        window: &Window,
        event: &game_loop::winit::event::Event<UscInputEvent>,
    ) {
        use game_loop::winit::event::*;
        if let Event::WindowEvent {
            window_id: _,
            event,
        } = event
        {
            if self.show_debug_ui || self.scenes.should_render_egui() {
                let event_response = self.gui.on_event(event);
                if event_response.consumed {
                    return;
                }
            }
        }

        let mut transformed_event = None;

        match event {
            Event::UserEvent(e) => {
                info!("{:?}", e);
                self.input_state.update(e);
                match e {
                    UscInputEvent::Laser(ls) => self.knob_state = *ls,
                    UscInputEvent::Button(b, s) => match s {
                        ElementState::Pressed => {
                            self.scenes.for_each_active_mut(|x| x.on_button_pressed(*b))
                        }
                        ElementState::Released => self
                            .scenes
                            .for_each_active_mut(|x| x.on_button_released(*b)),
                    },
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                self.mousex = position.x;
                self.mousey = position.y;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => self.scenes.clear(),
            Event::DeviceEvent {
                event:
                    DeviceEvent::Key(KeyboardInput {
                        virtual_keycode: Some(VirtualKeyCode::D),
                        state: ElementState::Pressed,
                        modifiers,
                        ..
                    }),
                ..
            } if modifiers.alt() => self.show_debug_ui = !self.show_debug_ui,
            Event::DeviceEvent {
                event:
                    DeviceEvent::Key(KeyboardInput {
                        virtual_keycode: Some(VirtualKeyCode::Return),
                        state: ElementState::Pressed,
                        modifiers,
                        ..
                    }),
                ..
            } if modifiers.alt() => self.toggle_fullscreen(window),
            Event::DeviceEvent {
                event:
                    DeviceEvent::Key(KeyboardInput {
                        scancode, state, ..
                    }),
                ..
            } => {
                if GameConfig::get().keyboard_buttons {
                    for button in GameConfig::get()
                        .keybinds
                        .iter()
                        .filter_map(|x| x.match_button(*scancode))
                    {
                        if self.input_state.is_button_held(button).is_none()
                            || *state == ElementState::Released
                        {
                            let button = UscInputEvent::Button(button, *state);
                            transformed_event = Some(Event::UserEvent(button));
                        }
                    }
                }
            }
            Event::DeviceEvent {
                event: game_loop::winit::event::DeviceEvent::MouseMotion { delta },
                ..
            } if GameConfig::get().mouse_knobs => {
                {
                    //TODO: Move somewhere else?
                    let s = window.inner_size();
                    _ = window
                        .set_cursor_position(PhysicalPosition::new(s.width / 2, s.height / 2));
                }

                let sens = GameConfig::get().mouse_ppr;
                let mut ls = LaserState::default();
                ls.update(kson::Side::Left, (delta.0 / sens) as _);
                ls.update(kson::Side::Right, (delta.1 / sens) as _);

                transformed_event = Some(Event::UserEvent(UscInputEvent::Laser(ls)));
            }
            _ => (),
        }

        if let Some(Event::UserEvent(e)) = transformed_event {
            self.input_state.update(&e);
            match e {
                UscInputEvent::Button(b, ElementState::Pressed) => {
                    self.scenes.for_each_active_mut(|x| x.on_button_pressed(b))
                }
                UscInputEvent::Button(b, ElementState::Released) => {
                    self.scenes.for_each_active_mut(|x| x.on_button_released(b))
                }
                UscInputEvent::Laser(_) => {}
            }
        }

        self.scenes
            .active
            .iter_mut()
            .filter(|x| !x.is_suspended())
            .for_each(|x| x.on_event(transformed_event.as_ref().unwrap_or(event)));
    }

    fn run_lua_gc(
        lua_arena: &Rc<RwLock<Arena<Rc<Lua>>>>,
        vgfx: &mut Vgfx,
        transition_lua_idx: Index,
        transition_song_lua_idx: Index,
    ) {
        profile_scope!("Garbage collect");
        lua_arena.write().unwrap().retain(|idx, lua| {
            //TODO: if reference count = 1, remove loaded gfx assets for state
            //lua.gc_collect();
            if Rc::strong_count(lua) > 1
                || idx == transition_lua_idx
                || idx == transition_song_lua_idx
            {
                LuaHttp::poll(lua);
                true
            } else {
                vgfx.drop_assets(lua_address(&lua));
                false
            }
        });
    }

    fn debug_ui(gui_context: &egui::Context, scenes: &mut Scenes) {
        profile_function!();
        if let Some(s) = scenes.active.last_mut() {
            s.debug_ui(gui_context);
        }
        puffin_egui::profiler_window(gui_context);
        egui::Window::new("Scenes").show(gui_context, |ui| {
            ui.label("Loaded");
            for ele in &scenes.loaded {
                ui.label(ele.name());
            }
            ui.separator();
            ui.label("Initialized");
            for ele in &scenes.initialized {
                ui.label(ele.name());
            }
            ui.separator();
            ui.label("Active");

            let mut closed_scene = None;

            for (i, ele) in scenes.active.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(ele.name());
                    if ui.button("Close").clicked() {
                        closed_scene = Some(i);
                    }
                });
            }

            if let Some(closed) = closed_scene {
                scenes.active.remove(closed);
            }

            if scenes.transition.is_some() {
                ui.label("Transitioning");
            }
        });
    }

    fn render_overlays(
        vgfx: &Arc<Mutex<Vgfx>>,
        frame_input: &td::FrameInput<()>,
        fps: f64,
        fps_paint: &vg::Paint,
    ) {
        profile_function!();
        let vgfx_lock = vgfx.try_lock();
        if let Ok(vgfx) = vgfx_lock {
            let mut canvas_lock = vgfx.canvas.try_lock();
            if let Ok(ref mut canvas) = canvas_lock {
                canvas.reset();
                canvas.fill_text(
                    frame_input.viewport.width as f32 - 5.0,
                    frame_input.viewport.height as f32 - 5.0,
                    format!("{:.1} FPS", fps),
                    fps_paint,
                );

                {
                    profile_scope!("Flush Canvas");
                    canvas.flush(); //also flushes game game ui, can take longer than it looks like it should
                }
            }
        }
    }

    fn update_game_data_and_clear(
        game_data: &Arc<Mutex<GameData>>,
        mousex: f64,
        mousey: f64,
        frame_input: &td::FrameInput<()>,
        laser_state: LaserState,
    ) {
        profile_function!();
        {
            let lock = game_data.lock();
            if let Ok(mut game_data) = lock {
                *game_data = GameData {
                    mouse_pos: (mousex, mousey),
                    resolution: (frame_input.viewport.width, frame_input.viewport.height),
                    profile_stack: std::mem::take(&mut game_data.profile_stack),
                    laser_state,
                    audio_samples: std::mem::take(&mut game_data.audio_samples),
                    audio_sample_play_status: std::mem::take(
                        &mut game_data.audio_sample_play_status,
                    ),
                };
            }
        }

        {
            frame_input
                .screen()
                .clear(td::ClearState::color_and_depth(0.0, 0.0, 0.0, 0.0, 1.0));
            // .render(&camera, [&model], &[]);
        }
    }

    fn reset_viewport_size(vgfx: Arc<Mutex<Vgfx>>, frame_input: &td::FrameInput<()>) {
        let vgfx_lock = vgfx.try_lock();
        if let Ok(vgfx) = vgfx_lock {
            let mut canvas_lock = vgfx.canvas.try_lock();
            if let Ok(ref mut canvas) = canvas_lock {
                canvas.reset();
                canvas.set_size(frame_input.viewport.width, frame_input.viewport.height, 1.0);
                canvas.flush();
            }
        }
    }

    fn toggle_fullscreen(&self, window: &Window) {
        match window.fullscreen() {
            Some(_) => window.set_fullscreen(None),
            None => window.set_fullscreen(Some(game_loop::winit::window::Fullscreen::Borderless(
                window.current_monitor(),
            ))),
        }
    }
}