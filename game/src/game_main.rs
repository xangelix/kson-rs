use std::{
    num::NonZeroU32,
    ops::{Add, Sub},
    rc::Rc,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, RwLock,
    },
    time::{Duration, SystemTime},
};

use di::{RefMut, ServiceProvider};
use egui_glow::EguiGlow;
use femtovg::Paint;
use game_loop::winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event,
    keyboard::{Key, NamedKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::Window,
};

use glutin::{
    context::PossiblyCurrentContext,
    surface::{GlSurface, SwapInterval},
};
use puffin::{profile_function, profile_scope};

use td::{FrameOutput, Modifiers};
use tealr::mlu::mlua::Lua;
use three_d::FrameInput;

use femtovg as vg;
use three_d as td;

use crate::{
    button_codes::{LaserState, UscInputEvent},
    config::{Fullscreen, GameConfig},
    game::{gauge::Gauge, HitRating},
    game_data::GameData,
    input_state::InputState,
    lua_http::LuaHttp,
    lua_service::LuaProvider,
    main_menu::MainMenuButton,
    scene,
    settings_screen::SettingsScreen,
    song_provider, songselect,
    transition::Transition,
    util::lua_address,
    vg_ui::Vgfx,
    window::find_monitor,
    worker_service::WorkerService,
    LuaArena, RuscMixer, Scenes, FRAME_ACC_SIZE,
};

pub enum AutoPlay {
    None,
    Buttons,
    Lasers,
    All,
}

pub enum ControlMessage {
    None,
    MainMenu(MainMenuButton),
    Song {
        song: Arc<songselect::Song>,
        diff: usize,
        loader: song_provider::LoadSongFn,
        autoplay: AutoPlay,
    },
    TransitionComplete(Box<dyn scene::Scene>),
    Result {
        song: Arc<songselect::Song>,
        diff_idx: usize,
        score: u32,
        gauge: Gauge,
        hit_ratings: Vec<HitRating>,
    },

    ApplySettings,
}

impl Default for ControlMessage {
    fn default() -> Self {
        Self::None
    }
}

pub struct GameMain {
    lua_arena: di::RefMut<LuaArena>,
    lua_provider: Arc<LuaProvider>,
    scenes: Scenes,
    pub control_tx: Sender<ControlMessage>,
    control_rx: Receiver<ControlMessage>,
    knob_state: LaserState,
    frame_times: [f64; 16],
    frame_time_index: usize,
    fps_paint: Paint,
    transition_lua: Rc<Lua>,
    transition_song_lua: Rc<Lua>,
    game_data: Arc<RwLock<GameData>>,
    vgfx: Arc<RwLock<Vgfx>>,
    frame_count: u32,
    gui: EguiGlow,
    show_debug_ui: bool,
    mousex: f64,
    mousey: f64,
    input_state: InputState,
    mixer: RuscMixer,
    modifiers: Modifiers,
    service_provider: ServiceProvider,
    show_fps: bool,
}

impl GameMain {
    pub fn new(
        scenes: Scenes,
        fps_paint: Paint,
        gui: EguiGlow,
        show_debug_ui: bool,
        service_provider: ServiceProvider,
    ) -> Self {
        let (control_tx, control_rx) = channel();
        Self {
            lua_arena: service_provider.get_required(),
            lua_provider: service_provider.get_required(),
            scenes,
            control_tx,
            control_rx,
            knob_state: LaserState::default(),
            frame_times: [0.01; 16],
            frame_time_index: 0,
            fps_paint,
            transition_lua: LuaProvider::new_lua(),
            transition_song_lua: LuaProvider::new_lua(),
            game_data: service_provider.get_required_mut(),
            vgfx: service_provider.get_required_mut(),
            frame_count: 0,
            gui,
            show_debug_ui,
            mousex: 0.0,
            mousey: 0.0,
            input_state: InputState::clone(&service_provider.get_required()),
            mixer: service_provider.get_required(),
            modifiers: Modifiers::default(),
            service_provider,
            show_fps: GameConfig::get().graphics.show_fps,
        }
    }

    const KEYBOARD_LASER_SENS: f32 = 1.0 / 240.0;
    pub fn update(&mut self) {
        {
            for ele in self.service_provider.get_all_mut::<dyn WorkerService>() {
                ele.write().expect("Worker service closed").update()
            }
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
                x.on_event(&event::Event::UserEvent(UscInputEvent::Laser(
                    ls,
                    SystemTime::now(),
                )))
            });
        }
    }
    pub fn render(
        &mut self,
        frame_input: FrameInput,
        window: &game_loop::winit::window::Window,
        surface: &glutin::surface::Surface<glutin::surface::WindowSurface>,
        gl_context: &PossiblyCurrentContext,
    ) -> FrameOutput {
        let GameMain {
            lua_arena,
            scenes,
            control_tx,
            control_rx,
            knob_state,
            frame_times,
            fps_paint,
            transition_lua,
            transition_song_lua,
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
            modifiers: _,
            service_provider,
            lua_provider,
            show_fps,
        } = self;

        knob_state.zero_deltas();
        puffin::profile_scope!("Frame");
        puffin::GlobalProfiler::lock().new_frame();

        for lua in lua_arena.read().expect("Lock error").0.iter() {
            lua.set_app_data(frame_input.clone());
        }
        let _lua_frame_input = frame_input.clone();
        let _lua_mixer = mixer.clone();

        if frame_input.first_frame {
            frame_input
                .screen()
                .clear(td::ClearState::color(0.0, 0.0, 0.0, 1.0));
            let vgfx = vgfx.write().expect("Lock error");
            let mut canvas = vgfx.canvas.lock().expect("Lock error");
            canvas.reset();
            canvas.set_size(frame_input.viewport.width, frame_input.viewport.height, 1.0);
            _ = canvas.fill_text(
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
            lua_provider
                .register_libraries(transition_lua.clone(), "transition.lua")
                .expect("Failed to register lua libraries");

            lua_provider
                .register_libraries(transition_song_lua.clone(), "songtransition.lua")
                .expect("Failed to register lua libraries");
            *frame_count += 1;
        }

        //Initialize loaded scenes
        scenes.tick(frame_input.elapsed_time, *knob_state, control_tx.clone());

        while let Ok(control_msg) = control_rx.try_recv() {
            match control_msg {
                ControlMessage::None => {}
                ControlMessage::MainMenu(b) => match b {
                    MainMenuButton::Start => {
                        scenes.suspend_top();

                        if let Ok(_arena) = lua_arena.read() {
                            let transition_lua = transition_lua.clone();
                            scenes.transition = Transition::new(
                                transition_lua,
                                ControlMessage::MainMenu(MainMenuButton::Start),
                                control_tx.clone(),
                                vgfx.clone(),
                                frame_input.viewport,
                                service_provider.create_scope(),
                            )
                            .ok()
                        }
                    }
                    MainMenuButton::Downloads => {}
                    MainMenuButton::Exit => {
                        scenes.clear();
                    }
                    MainMenuButton::Options => scenes.loaded.push(Box::new(SettingsScreen::new(
                        service_provider.create_scope(),
                        control_tx.clone(),
                        window,
                    ))),
                    _ => {}
                },
                ControlMessage::Song {
                    diff,
                    loader,
                    song,
                    autoplay,
                } => {
                    if let Ok(_arena) = lua_arena.read() {
                        let transition_lua = transition_song_lua.clone();
                        scenes.transition = Transition::new(
                            transition_lua,
                            ControlMessage::Song {
                                diff,
                                loader,
                                song,
                                autoplay,
                            },
                            control_tx.clone(),
                            vgfx.clone(),
                            frame_input.viewport,
                            service_provider.create_scope(),
                        )
                        .ok()
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
                    if let Ok(_arena) = lua_arena.read() {
                        let transition_lua = transition_lua.clone();
                        scenes.transition = Transition::new(
                            transition_lua,
                            ControlMessage::Result {
                                song,
                                diff_idx,
                                score,
                                gauge,
                                hit_ratings,
                            },
                            control_tx.clone(),
                            vgfx.clone(),
                            frame_input.viewport,
                            service_provider.create_scope(),
                        )
                        .ok()
                    }
                }
                ControlMessage::ApplySettings => {
                    //TODO: Reload skin
                    let settings = GameConfig::get();
                    _ = surface.set_swap_interval(
                        gl_context,
                        if settings.graphics.vsync {
                            SwapInterval::Wait(NonZeroU32::new(1).expect("Invalid value"))
                        } else {
                            SwapInterval::DontWait
                        },
                    );

                    *show_fps = settings.graphics.show_fps;

                    window.set_fullscreen(match settings.graphics.fullscreen {
                        Fullscreen::Windowed { .. } => None,
                        Fullscreen::Borderless { monitor } => {
                            let m = find_monitor(window.available_monitors(), monitor);
                            Some(game_loop::winit::window::Fullscreen::Borderless(m))
                        }
                        Fullscreen::Exclusive {
                            monitor,
                            resolution,
                        } => {
                            let m =
                                find_monitor(window.available_monitors(), monitor).and_then(|m| {
                                    m.video_modes()
                                        .filter(|x| x.size() == resolution)
                                        .max_by_key(|x| x.refresh_rate_millihertz())
                                });

                            m.map(game_loop::winit::window::Fullscreen::Exclusive)
                        }
                    });

                    let sink = service_provider.get_required::<rodio::Sink>();
                    sink.set_volume(settings.master_volume);
                }
            }
        }

        frame_times[*frame_time_index] = frame_input.elapsed_time;
        *frame_time_index = (*frame_time_index + 1) % FRAME_ACC_SIZE;
        let fps = 1000_f64 / (frame_times.iter().sum::<f64>() / FRAME_ACC_SIZE as f64);

        Self::update_game_data_and_clear(
            game_data,
            *mousex,
            *mousey,
            &frame_input,
            self.input_state.clone(),
        );

        scenes.render(frame_input.clone(), vgfx);
        Self::render_overlays(vgfx, &frame_input, fps, fps_paint, *show_fps);

        gui.run(window, |ctx| {
            scenes.render_egui(ctx);

            if *show_debug_ui {
                Self::debug_ui(ctx, scenes);
            }
        });
        gui.paint(window);

        Self::run_lua_gc(lua_arena, &mut vgfx.write().expect("Lock error"));

        if let Ok(mut a) = game_data.write() {
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
                let event_response = self.gui.on_window_event(window, event);
                if event_response.consumed {
                    return;
                }
            }
        }

        let mut transformed_event = None;

        let (offset, offset_neg) = {
            let global_offset = GameConfig::get().global_offset;
            (
                Duration::from_millis(global_offset.unsigned_abs() as _),
                global_offset < 0,
            )
        };
        let text_input_active = self.input_state.text_input_active();

        //TODO: Refactor keyboard handling
        match event {
            Event::UserEvent(e) => {
                self.input_state.update(e);
                match e {
                    UscInputEvent::Laser(ls, _time) => self.knob_state = *ls,
                    UscInputEvent::Button(b, s, time) => match s {
                        ElementState::Pressed => self
                            .scenes
                            .for_each_active_mut(|x| x.on_button_pressed(*b, *time)),
                        ElementState::Released => self
                            .scenes
                            .for_each_active_mut(|x| x.on_button_released(*b, *time)),
                    },
                }
            }
            Event::WindowEvent {
                window_id: _,
                event: WindowEvent::Resized(physical_size),
            } => {
                let windowed = &mut GameConfig::get_mut().graphics.fullscreen;
                if let Fullscreen::Windowed { size, .. } = windowed {
                    *size = *physical_size;
                }
                self.reset_viewport_size(physical_size)
            }
            Event::WindowEvent {
                window_id: _,
                event: WindowEvent::Moved(physical_pos),
            } => {
                let windowed = &mut GameConfig::get_mut().graphics.fullscreen;
                if let Fullscreen::Windowed { pos, .. } = windowed {
                    *pos = *physical_pos;
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
                event: WindowEvent::ModifiersChanged(mods),
                ..
            } => {
                self.modifiers = three_d::renderer::control::Modifiers {
                    alt: mods.state().alt_key(),
                    ctrl: mods.state().control_key(),
                    shift: mods.state().shift_key(),
                    command: mods.state().super_key(),
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => self.scenes.clear(),
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event: key, .. },
                ..
            } if key.state == ElementState::Pressed
                && key.key_without_modifiers() == Key::Character("d".into())
                && self.modifiers.alt
                && !text_input_active =>
            {
                self.show_debug_ui = !self.show_debug_ui
            }
            Event::WindowEvent {
                event:
                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                logical_key: Key::Named(NamedKey::Enter),
                                state: ElementState::Pressed,
                                ..
                            },
                        ..
                    },
                ..
            } if self.modifiers.alt && !text_input_active => self.toggle_fullscreen(window),
            Event::WindowEvent {
                event:
                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                physical_key,
                                state,
                                ..
                            },
                        ..
                    },
                ..
            } => {
                if !text_input_active && GameConfig::get().keyboard_buttons {
                    for button in GameConfig::get()
                        .keybinds
                        .iter()
                        .filter_map(|x| x.match_button(*physical_key))
                    {
                        if self.input_state.is_button_held(button).is_none()
                            || *state == ElementState::Released
                        {
                            let button = UscInputEvent::Button(
                                button,
                                *state,
                                if offset_neg {
                                    SystemTime::now().add(offset)
                                } else {
                                    SystemTime::now().sub(offset)
                                },
                            );
                            transformed_event = Some(Event::UserEvent(button));
                        }
                    }
                }
            }
            Event::DeviceEvent {
                event: game_loop::winit::event::DeviceEvent::MouseMotion { delta },
                ..
            } if !text_input_active && GameConfig::get().mouse_knobs => {
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

                transformed_event = Some(Event::UserEvent(UscInputEvent::Laser(
                    ls,
                    SystemTime::now().sub(offset),
                )));
            }
            _ => (),
        }

        if let Some(Event::UserEvent(e)) = transformed_event {
            self.input_state.update(&e);
            match e {
                UscInputEvent::Button(b, ElementState::Pressed, time) => self
                    .scenes
                    .for_each_active_mut(|x| x.on_button_pressed(b, time)),
                UscInputEvent::Button(b, ElementState::Released, time) => self
                    .scenes
                    .for_each_active_mut(|x| x.on_button_released(b, time)),
                UscInputEvent::Laser(_, _) => {}
            }
        }

        self.scenes
            .active
            .iter_mut()
            .filter(|x| !x.is_suspended())
            .for_each(|x| x.on_event(transformed_event.as_ref().unwrap_or(event)));
    }

    fn run_lua_gc(lua_arena: &mut RefMut<LuaArena>, vgfx: &mut Vgfx) {
        profile_scope!("Garbage collect");
        lua_arena.write().expect("Lock error").0.retain(|lua| {
            //lua.gc_collect();
            if Rc::strong_count(lua) > 1 {
                LuaHttp::poll(lua);
                true
            } else {
                vgfx.drop_assets(lua_address(lua));
                false
            }
        });
    }

    fn debug_ui(gui_context: &egui::Context, scenes: &mut Scenes) {
        profile_function!();
        if let Some(s) = scenes.active.last_mut() {
            crate::log_result!(s.debug_ui(gui_context));
        }
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
        vgfx: &Arc<RwLock<Vgfx>>,
        frame_input: &td::FrameInput,
        fps: f64,
        fps_paint: &vg::Paint,
        show_fps: bool,
    ) {
        profile_function!();
        let vgfx_lock = vgfx.write();
        if let Ok(vgfx) = vgfx_lock {
            let mut canvas_lock = vgfx.canvas.try_lock();
            if let Ok(ref mut canvas) = canvas_lock {
                canvas.reset();
                if show_fps {
                    _ = canvas.fill_text(
                        frame_input.viewport.width as f32 - 5.0,
                        frame_input.viewport.height as f32 - 5.0,
                        format!("{:.1} FPS", fps),
                        fps_paint,
                    );
                }

                {
                    profile_scope!("Flush Canvas");
                    canvas.flush(); //also flushes game game ui, can take longer than it looks like it should
                }
            }
        }
    }

    fn update_game_data_and_clear(
        game_data: &Arc<RwLock<GameData>>,
        mousex: f64,
        mousey: f64,
        frame_input: &td::FrameInput,
        input_state: InputState,
    ) {
        profile_function!();
        {
            let lock = game_data.write();
            if let Ok(mut game_data) = lock {
                *game_data = GameData {
                    mouse_pos: (mousex, mousey),
                    resolution: (frame_input.viewport.width, frame_input.viewport.height),
                    profile_stack: std::mem::take(&mut game_data.profile_stack),
                    input_state,
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
                .clear(td::ClearState::color_and_depth(0.0, 0.0, 0.0, 1.0, 1.0));
            // .render(&camera, [&model], &[]);
        }
    }

    fn reset_viewport_size(&self, size: &PhysicalSize<u32>) {
        let vgfx_lock = self.vgfx.write();
        if let Ok(vgfx) = vgfx_lock {
            let mut canvas_lock = vgfx.canvas.try_lock();
            if let Ok(ref mut canvas) = canvas_lock {
                canvas.reset();
                canvas.set_size(size.width, size.height, 1.0);
                canvas.flush();
            }
        }
    }

    fn toggle_fullscreen(&self, window: &Window) {
        let fullscreen = &mut GameConfig::get_mut().graphics.fullscreen;
        match window.fullscreen() {
            Some(_) => {
                window.set_fullscreen(None);
                *fullscreen = Fullscreen::Windowed {
                    pos: window
                        .outer_position()
                        .unwrap_or(PhysicalPosition::new(0, 0)),
                    size: window.inner_size(),
                }
            }
            None => {
                let current_monitor = window.current_monitor();

                if let Some(m) = current_monitor.as_ref() {
                    *fullscreen = Fullscreen::Borderless {
                        monitor: m.position(),
                    };
                }

                window.set_fullscreen(Some(game_loop::winit::window::Fullscreen::Borderless(
                    current_monitor,
                )))
            }
        }
    }
}
