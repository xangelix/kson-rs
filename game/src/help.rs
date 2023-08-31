use std::sync::{Arc, Mutex};

use generational_arena::Index;
use tealr::{
    mlu::{
        mlua::{self, FromLuaMulti, Lua, Result, ToLuaMulti},
        MaybeSend,
    },
    TealMultiValue, TypeName,
};

pub(crate) fn add_lua_static_method<'lua, M, A, R, F, T: 'static + Sized + TypeName>(
    methods: &mut M,
    name: &'static str,
    mut function: F,
) where
    M: Sized + tealr::mlu::TealDataMethods<'lua, T>,
    A: FromLuaMulti<'lua> + TealMultiValue,
    R: ToLuaMulti<'lua> + TealMultiValue,
    F: 'static + MaybeSend + FnMut(&'lua Lua, &mut T, A) -> Result<R>,
{
    methods.add_function_mut(name, move |lua, p: A| {
        let _profile_scope = if puffin::are_scopes_on() && !name.ends_with("Profile") {
            Some(puffin::ProfilerScope::new(
                name,
                &format!(
                    "{}:{}",
                    lua.inspect_stack(1)
                        .and_then(|s| s
                            .source()
                            .source
                            .map(|s| String::from_utf8_lossy(s).to_string()))
                        .unwrap_or_default(),
                    lua.inspect_stack(1).map(|s| s.curr_line()).unwrap_or(-1)
                ),
                "",
            ))
        } else {
            None
        };

        let mut maybe_data = { lua.app_data_ref::<Arc<Mutex<T>>>().map(|x| x.clone()) };
        if let Some(data_arc) = maybe_data.take() {
            let data = data_arc.clone();
            drop(data_arc);
            drop(maybe_data);
            let data_lock = data.try_lock();
            if let Ok(mut data) = data_lock {
                function(lua, &mut data, p)
            } else {
                Err(mlua::Error::external("App data not set"))
            }
        } else {
            Err(mlua::Error::external("App data not set"))
        }
    })
}