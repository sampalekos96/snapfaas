use lmdb::{Cursor, RoTransaction, Transaction};
use rlua::prelude::*;

pub(crate) struct LuaTxn<'env>(pub &'env RoTransaction<'env>, pub &'env lmdb::Database);

impl<'env> rlua::UserData for LuaTxn<'env> {
    fn add_methods<'lua, M: rlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("get", |ctx, this, key: bstr::BString| {
            match this.0.get(*this.1, &key.as_slice()) {
                Ok(val) => {
                    let r: &bstr::BStr = val.into();
                    r.to_lua(ctx)
                },
                Err(_) => {
                    Ok(LuaValue::Nil)
                }
            }
        });

        methods.add_method("cursor", |ctx, this, key: bstr::BString| {
            struct It<'txn>(lmdb::Iter<'txn>);
            unsafe impl<'a> Send for It<'a> {} // TODO, this is annoying because we _don't_ use across threads
            match this.0.open_ro_cursor(*this.1).map(|mut c| c.iter_from(&key.as_slice())) {
                Ok(it) => {
                    let mut it = It(it);
                    ctx.create_function_mut(move |ctx, _: ()| {
                        match it.0.next() {
                            Some(Ok((key, val))) => {
                                let k: &bstr::BStr = key.into();
                                let v: &bstr::BStr = val.into();
                                (k, v).to_lua_multi(ctx)
                            },
                            _ => LuaValue::Nil.to_lua_multi(ctx)
                        }
                    }).map(LuaValue::Function)
                },
                Err(_) => {
                    Ok(LuaValue::Nil)
                }
            }
        });
    }
}

fn lua_val_to_json_val(input: LuaValue) -> serde_json::Value {
    match input {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(b) => serde_json::Value::Bool(b),
        LuaValue::Integer(i) => serde_json::Value::Number(i.into()),
        LuaValue::Number(n) => {
            serde_json::Number::from_f64(n).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null)
        }
        LuaValue::String(s) => s.to_str().map(String::from).map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
        LuaValue::Table(table) => {
            if table.contains_key(1u32).unwrap_or(false) {
                // assume it's an array
                serde_json::Value::Array(table.sequence_values::<LuaValue>().filter_map(Result::ok).map(lua_val_to_json_val).collect())
            } else {
                let mut obj: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
                for (k, v) in table.pairs().filter_map(Result::ok) {
                    obj.insert(k, lua_val_to_json_val(v));
                }
                serde_json::Value::Object(obj)
            }
        },
        _ => serde_json::Value::Null,
    }
}

pub(crate) fn get_lua<'env>(query: &Vec<u8>, txn: LuaTxn<'env>) -> Result<serde_json::Value, LuaError> {
    let lua = Lua::new();
    let results = lua.context(move |context| {
        context.scope(|scope| {
            context.globals().set("db", scope.create_nonstatic_userdata(txn)?)?;
            Ok(lua_val_to_json_val(context.load(query).eval()?))
            //let result: LuaTable = context.load(query).eval()?;
            //Ok(result.sequence_values().filter_map(Result::ok).collect())
        })
    })?;
    Ok(serde_json::json!({
        "results": results
    }))
}
