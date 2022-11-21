use std::{
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    game_data::{ExportGame, GameData},
    vg_ui::{ExportVgfx, Vgfx},
};
use femtovg as vg;
use generational_arena::Arena;
use gilrs::Mapping;
use log::*;
use main_menu::MainMenuButton;
use puffin::profile_scope;
use songselect::SongSelect;
use td::egui;
use td::HasContext;
use tealr::mlu::{
    mlua::{Function, Lua},
    UserDataProxy,
};
use three_d as td;

mod button_codes;
mod game_data;
mod help;
mod main_menu;
mod scene;
mod songselect;
mod vg_ui;
pub enum ControlMessage {
    MainMenu(MainMenuButton),
    Song(PathBuf),
    Result {
        song: songselect::Song,
        diff_idx: usize,
        score: u32,
    },
}

fn main() -> anyhow::Result<()> {
    puffin::set_scopes_on(true);
    let server_addr = format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT);
    let _server = puffin_http::Server::new(&server_addr)?;

    let window = td::Window::new(td::WindowSettings {
        title: "Test".to_string(),
        max_size: None,
        multisamples: 4,
        vsync: false,
        ..Default::default()
    })
    .unwrap();

    simple_logger::init_with_level(Level::Info)?;

    let mut input = gilrs::GilrsBuilder::default()
        .add_included_mappings(true)
        .add_mappings("03000000d01600006d0a000000000000,Pocket Voltex Rev4,a:b1,b:b2,y:b3,x:b4,leftshoulder:b5,rightshoulder:b6,start:b0,leftx:a0,rightx:a1")
        .build()
        .expect("Failed to create input context");

    while input.next_event().is_some() {} //empty events

    let context = window.gl();
    let renderer = unsafe {
        vg::renderer::OpenGl::new_from_context(
            std::mem::transmute_copy(&**context),
            context.version().is_embedded,
        )
        .expect("awd")
    };

    let canvas = Arc::new(Mutex::new(
        vg::Canvas::new(renderer).expect("Failed to create canvas"),
    ));
    let mut vgfx = Arc::new(Mutex::new(vg_ui::Vgfx::new(
        canvas.clone(),
        std::env::current_dir()?,
    )));

    // Create a CPU-side mesh consisting of a single colored triangle
    let positions = vec![
        td::vec3(0.5, -0.5, 0.0),  // bottom right
        td::vec3(-0.5, -0.5, 0.0), // bottom left
        td::vec3(0.0, 0.5, 0.0),   // top
    ];
    let colors = vec![
        td::Color::new(255, 0, 0, 255), // bottom right
        td::Color::new(0, 255, 0, 255), // bottom left
        td::Color::new(0, 0, 255, 255), // top
    ];
    let cpu_mesh = td::CpuMesh {
        positions: td::Positions::F32(positions),
        colors: Some(colors),
        ..Default::default()
    };

    // Construct a model, with a default color material, thereby transferring the mesh data to the GPU
    let mut model = td::Gm::new(
        td::Mesh::new(&context, &cpu_mesh),
        td::ColorMaterial::default(),
    );

    let mut camera = td::Camera::new_perspective(
        window.viewport(),
        td::vec3(0.0, 0.0, 2.0),
        td::vec3(0.0, 0.0, 0.0),
        td::vec3(0.0, 1.0, 0.0),
        td::degrees(45.0),
        0.1,
        10.0,
    );

    let mut mousex = 0.0;
    let mut mousey = 0.0;

    let songs_folder = loop {
        if let Some(f) = rfd::FileDialog::new().pick_folder() {
            break f;
        }
    };

    let typedef_folder = Path::new("types");
    if !typedef_folder.exists() {
        std::fs::create_dir_all(typedef_folder)?;
    }

    let gfx_typedef = tealr::TypeWalker::new()
        .process_type_inline::<vg_ui::Vgfx>()
        .generate_global("gfx")?;

    let game_typedef = tealr::TypeWalker::new()
        .process_type_inline::<game_data::GameData>()
        .generate_global("game")?;

    let songwheel_typedef = tealr::TypeWalker::new()
        .process_type::<songselect::Song>()
        .process_type::<songselect::Difficulty>()
        .process_type_inline::<songselect::SongSelect>()
        .generate_global("songwheel")?;

    let mut typedef_file_path = typedef_folder.to_path_buf();
    typedef_file_path.push("rusc.d.tl");
    let mut typedef_file = std::fs::File::create(typedef_file_path).expect("Failed to create");
    let file_content = format!("{}\n{}\n{}", gfx_typedef, game_typedef, songwheel_typedef)
        .lines()
        .filter(|l| !l.starts_with("return"))
        .collect::<Vec<_>>()
        .join("\n");

    write!(typedef_file, "{}", file_content)?;
    typedef_file.flush()?;
    drop(typedef_file);
    let mut gui = three_d::GUI::new(&context);

    const FRAME_ACC_SIZE: usize = 16;
    let mut frame_times = [16.0; FRAME_ACC_SIZE];
    let mut frame_time_index = 0;
    let fps_paint = vg::Paint::color(vg::Color::white()).with_text_align(vg::Align::Right);

    let mut scenes_loaded: Vec<Box<dyn scene::Scene>> = vec![]; //Uninitialized
    let mut scenes: Vec<Box<dyn scene::Scene>> = vec![]; //Initialized

    scenes_loaded.push(Box::new(main_menu::MainMenu::new()));
    let game_data = Arc::new(Mutex::new(game_data::GameData {
        mouse_pos: (mousex, mousey),
        resolution: (800, 600),
        profile_stack: vec![],
    }));

    let lua_arena: Rc<RwLock<Arena<Rc<Lua>>>> = Rc::new(RwLock::new(Arena::new()));

    let (control_tx, control_rx) = std::sync::mpsc::channel();
    window.render_loop(move |mut frame_input| {
        puffin::profile_scope!("Frame");
        puffin::GlobalProfiler::lock().new_frame();

        let load_lua = |game_data: Arc<Mutex<GameData>>,
                        vgfx: Arc<Mutex<Vgfx>>,
                        arena: Rc<RwLock<Arena<Rc<Lua>>>>| {
            Box::new(move |lua: Rc<Lua>, script_path| {
                tealr::mlu::set_global_env(ExportVgfx, &lua)?;
                tealr::mlu::set_global_env(ExportGame, &lua)?;

                let idx = arena
                    .write()
                    .expect("Could not get lock to lua arena")
                    .insert(lua.clone());
                lua.set_app_data(vgfx.clone());
                lua.set_app_data(game_data.clone());
                lua.set_app_data(idx);
                lua.gc_stop();
                let mut real_script_path = std::env::current_dir()?;
                real_script_path.push("scripts");
                real_script_path.push(script_path);
                let test_code = std::fs::read_to_string(real_script_path)?;
                lua.load(&test_code).set_name(script_path)?.eval::<()>()?;
                Ok(())
            })
        };

        //Initialize loaded scenes
        scenes_loaded.retain_mut(|s| {
            match s.init(
                load_lua(game_data.clone(), vgfx.clone(), lua_arena.clone()),
                control_tx.clone(),
            ) {
                Ok(_) => true,
                Err(e) => {
                    error!("{:?}", e);
                    false
                }
            }
        });
        scenes.append(&mut scenes_loaded);

        while let Ok(control_msg) = control_rx.try_recv() {
            match control_msg {
                ControlMessage::MainMenu(b) => match b {
                    MainMenuButton::Start => {
                        scenes_loaded
                            .push(Box::new(songselect::SongSelectScene::new(&songs_folder)));
                    }
                    MainMenuButton::Downloads => {}
                    _ => {}
                },
                ControlMessage::Song(p) => info!("{:?}", p),
                ControlMessage::Result {
                    song,
                    diff_idx,
                    score,
                } => todo!(),
            }
        }

        camera.set_viewport(frame_input.viewport);
        // Set the current transformation of the triangle
        model.set_transformation(td::Mat4::from_angle_y(td::radians(
            (frame_input.accumulated_time * 0.005) as f32,
        )));

        frame_times[frame_time_index as usize] = frame_input.elapsed_time;
        frame_time_index = (frame_time_index + 1) % FRAME_ACC_SIZE;
        let fps = 1000_f64 / (frame_times.iter().sum::<f64>() / FRAME_ACC_SIZE as f64);

        for event in &mut frame_input.events {
            if let td::Event::MouseMotion {
                button: _,
                delta: _,
                position,
                modifiers: _,
                handled: _,
            } = *event
            {
                (mousex, mousey) = position;
            }

            for scene in scenes.iter_mut().filter(|s| !s.is_suspended()) {
                scene.on_event(event); //TODO: break on event handled
            }
        }

        while let Some(e) = input.next_event() {
            match e.event {
                gilrs::EventType::ButtonPressed(button, _) => {
                    let button = button_codes::UscButton::from(button);
                    info!("{:?}", button);
                    scenes
                        .iter_mut()
                        .filter(|s| !s.is_suspended())
                        .for_each(|s| s.on_button_pressed(button))
                }
                gilrs::EventType::ButtonRepeated(_, _) => {}
                gilrs::EventType::ButtonReleased(_, _) => {}
                gilrs::EventType::ButtonChanged(_, _, _) => {}
                gilrs::EventType::AxisChanged(axis, value, code) => {
                    info!("{:?}, {:.3}, {:?}", axis, value, code)
                }
                gilrs::EventType::Connected => {}
                gilrs::EventType::Disconnected => {}
                gilrs::EventType::Dropped => {}
            }
        }

        if frame_input.first_frame {
            input.gamepads().for_each(|(_, g)| {
                info!("{} uuid: {}", g.name(), uuid::Uuid::from_bytes(g.uuid()))
            });
        }

        {
            let lock = game_data.lock();
            if let Ok(mut game_data) = lock {
                *game_data = GameData {
                    mouse_pos: (mousex, mousey),
                    resolution: (frame_input.viewport.width, frame_input.viewport.height),
                    profile_stack: std::mem::take(&mut game_data.profile_stack),
                };
            }
        }

        {
            frame_input
                .screen()
                .clear(td::ClearState::color_and_depth(0.0, 0.0, 0.0, 0.0, 1.0));
            // .render(&camera, [&model], &[]);
        }

        {
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

        {
            profile_scope!("Tick");
            scenes.retain_mut(|s| match s.tick(frame_input.elapsed_time) {
                Ok(close) => !close,
                Err(e) => {
                    error!("{:?}", e);
                    false
                }
            });
        }
        {
            profile_scope!("Render");
            scenes.retain_mut(|s| {
                if s.is_suspended() {
                    true
                } else {
                    match s.render(frame_input.elapsed_time) {
                        Ok(close) => !close,
                        Err(e) => {
                            error!("{:?}", e);
                            false
                        }
                    }
                }
            })
        }

        {
            let vgfx_lock = vgfx.try_lock();
            if let Ok(vgfx) = vgfx_lock {
                let mut canvas_lock = vgfx.canvas.try_lock();
                if let Ok(ref mut canvas) = canvas_lock {
                    canvas.reset();
                    canvas.set_size(frame_input.viewport.width, frame_input.viewport.height, 1.0);
                    canvas.fill_text(
                        frame_input.viewport.width as f32 - 5.0,
                        frame_input.viewport.height as f32 - 5.0,
                        format!("{:.1} FPS", fps),
                        &fps_paint,
                    );
                    canvas.flush();
                }
            }
        }

        {
            profile_scope!("Debug UI");
            gui.update(
                &mut frame_input.events,
                frame_input.accumulated_time,
                frame_input.viewport,
                frame_input.device_pixel_ratio,
                |gui_context| {
                    if let Some(s) = scenes.last_mut() {
                        s.debug_ui(gui_context);
                    }
                },
            );

            frame_input.screen().write(|| gui.render());
        }

        {
            profile_scope!("Garbage collect");
            for (idx, lua) in lua_arena.read().unwrap().iter() {
                //TODO: if reference count = 1, remove loaded gfx assets for state
                lua.gc_collect();
                lua.gc_collect();
            }
        }

        {
            game_data.lock().map(|mut a| a.profile_stack.clear());
        }

        td::FrameOutput {
            exit: scenes.is_empty() && scenes_loaded.is_empty(),
            swap_buffers: true,
            wait_next_event: false,
        }
    });

    Ok(())
}
