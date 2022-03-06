use std::collections::BTreeMap;
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

        methods.add_method("get_json", |ctx, this, key: bstr::BString| {
            let r = Ok(this.0.get(*this.1, &key.as_slice()).ok()
                .and_then(|val| serde_json::from_slice(val).ok())
                .map(|val| json_val_to_lua_val(val, ctx))
                .and_then(|r: LuaValue| r.to_lua(ctx).ok())
                .unwrap_or(LuaValue::Nil));
            r
        });

        methods.add_method("cursor", |ctx, this, key: bstr::BString| {
            struct It<'txn>(lmdb::Iter<'txn>);
            unsafe impl<'a> Send for It<'a> {} // TODO, this is annoying because we _don't_ use across threads
            match this.0.open_ro_cursor(*this.1).map(|mut c| c.iter_from(&key.as_slice())) {
                Ok(it) => {
                    let mut it = It(it);
                    ctx.create_function_mut(move |ctx, _: ()| {
                        match it.0.next() {
                            Some(Ok((key, _))) => {
                                let k: bstr::BString = key.into();
                                (k, LuaValue::Nil).to_lua_multi(ctx)
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

fn json_val_to_lua_val<'a>(input: serde_json::Value, ctx: LuaContext<'a>) -> LuaValue<'a> {
    match input {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(b)),
        serde_json::Value::Number(i) => i.as_f64().to_lua(ctx),
        serde_json::Value::String(s) => s.to_lua(ctx),
        serde_json::Value::Array(a) => a.iter().map(|e| json_val_to_lua_val(e.clone(), ctx)).collect::<Vec<LuaValue<'a>>>().to_lua(ctx),
        serde_json::Value::Object(a) => a.iter().map(|(k,v)| (k.clone(), json_val_to_lua_val(v.clone(), ctx))).collect::<BTreeMap<String, LuaValue<'a>>>().to_lua(ctx),
    }.unwrap_or(LuaValue::Nil)
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
    lua.set_hook(LuaHookTriggers {
            every_nth_instruction: Some(100000), ..Default::default()
    }, |_lua_context, debug| {
            Err(LuaError::RuntimeError(String::from("Too long")))
            //println!("1");
            //Ok(())
    });
    lua.set_memory_limit(Some(1024 * 1024));
    let results = lua.context(move |context| {
        context.scope(|scope| {
            context.globals().set("db", scope.create_nonstatic_userdata(txn)?)?;
            Ok(lua_val_to_json_val(context.load(query).eval()?))
        })
    })?;
    Ok(serde_json::json!({
        "results": results
    }))
}
