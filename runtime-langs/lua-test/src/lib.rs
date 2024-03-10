use std::marker::PhantomData;
use mlua::{AnyUserData, MetaMethod, MultiValue, UserData, UserDataRef};
use mlua::prelude::*;


trait Foo {}

trait LuaFoo {
    fn get_foo(&self) -> Box<dyn Foo>;
}


pub struct Setup<T> {
    phantom: PhantomData<T>
}

impl<T> Setup<T> {
    #[allow(unused_variables)]
    pub fn new(foo: Box<dyn Foo>) { Self { phantom: Default::default() }; }
}

impl<T: Foo> UserData for Setup<T> {}



#[cfg(test)]
mod tests {
    use mlua::{AnyUserData, Lua};
    use crate::{Foo, Setup};

    #[test]
    fn main() {
        let lua = Lua::new();
        let globals = lua.globals();

        //lua.register_userdata_type::<Box<dyn Foo>>(lua.registry_value()).unwrap();

        globals.set("new_foo", lua.create_function(|_, ud: AnyUserData| {
            let foo = ud.take::<Box<dyn Foo>>().unwrap();

            Ok(Setup::<dyn Foo>::new(foo))
        }).unwrap()).unwrap();

        lua.load("


        ").exec().unwrap();
    }
}

