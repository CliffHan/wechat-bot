use anyhow::{anyhow, Result};
use libloading::{Library, Symbol};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;

// check sdk API definition from wcf/include/sdk.h
const SDK_DLL: &str = "sdk.dll";
const WX_INIT_SDK: &str = "WxInitSDK";
const WX_DESTROY_SDK: &str = "WxDestroySDK";
type FnWxInitSDK = unsafe extern "C" fn(bool, i32) -> i32;
type FnWxDestroySDK = unsafe extern "C" fn() -> i32;

static DLL_LOADED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SDK_LIB: OnceCell<Library> = OnceCell::new();
static SDK_FN_INIT_SDK: OnceCell<Symbol<FnWxInitSDK>> = OnceCell::new();
static SDK_FN_DESTROY_SDK: OnceCell<Symbol<FnWxDestroySDK>> = OnceCell::new();

pub fn load_sdk_dll() -> Result<bool> {
    let mut loaded = DLL_LOADED.lock();
    if *loaded {
        return Ok(false); // load only once
    }
    unsafe {
        SDK_LIB.set(Library::new(SDK_DLL)?).map_err(|_| anyhow!("failed to save {}", SDK_DLL))?;
        let lib = SDK_LIB.get().unwrap();
        let fn_wx_init_sdk = lib.get(WX_INIT_SDK.as_bytes())?;
        let fn_wx_destroy_sdk = lib.get(WX_DESTROY_SDK.as_bytes())?;
        SDK_FN_INIT_SDK.set(fn_wx_init_sdk).map_err(|_| anyhow!("failed to set {}", WX_INIT_SDK))?;
        SDK_FN_DESTROY_SDK.set(fn_wx_destroy_sdk).map_err(|_| anyhow!("failed to set {}", WX_DESTROY_SDK))?;
    }
    *loaded = true;
    Ok(true)
}

pub fn wx_init_sdk(debug: bool, port: i32) -> Result<i32> {
    let target_fn = SDK_FN_INIT_SDK.get().ok_or_else(|| anyhow!("no WxInitSDK fn, sdk dll not load"))?;
    let result = unsafe { target_fn(debug, port) };
    Ok(result)
}

pub fn wx_destroy_sdk() -> Result<i32> {
    let target_fn = SDK_FN_DESTROY_SDK.get().ok_or_else(|| anyhow!("no WxDestroySDK fn, sdk dll not load"))?;
    let result = unsafe { target_fn() };
    Ok(result)
}
