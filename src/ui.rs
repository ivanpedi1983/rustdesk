use std::{
    collections::HashMap,
    iter::FromIterator,
    process::Child,
    sync::{Arc, Mutex},
};

use sciter::Value;

use hbb_common::{
    allow_err,
    config::{LocalConfig, PeerConfig},
    log,
};

#[cfg(not(any(feature = "flutter", feature = "cli")))]
use crate::ui_session_interface::Session;
use crate::{common::get_app_name, ipc, ui_interface::*};

mod cm;
#[cfg(feature = "inline")]
pub mod inline;
pub mod remote;

pub type Children = Arc<Mutex<(bool, HashMap<(String, String), Child>)>>;
#[allow(dead_code)]
type Status = (i32, bool, i64, String);

lazy_static::lazy_static! {
    // stupid workaround for https://sciter.com/forums/topic/crash-on-latest-tis-mac-sdk-sometimes/
    static ref STUPID_VALUES: Mutex<Vec<Arc<Vec<Value>>>> = Default::default();
}

#[cfg(not(any(feature = "flutter", feature = "cli")))]
lazy_static::lazy_static! {
    pub static ref CUR_SESSION: Arc<Mutex<Option<Session<remote::SciterHandler>>>> = Default::default();
    static ref CHILDREN : Children = Default::default();
}

struct UIHostHandler;

pub fn start(args: &mut [String]) {
    #[cfg(target_os = "macos")]
    crate::platform::delegate::show_dock();
    #[cfg(all(target_os = "linux", feature = "inline"))]
    {
        #[cfg(feature = "appimage")]
        let prefix = std::env::var("APPDIR").unwrap_or("".to_string());
        #[cfg(not(feature = "appimage"))]
        let prefix = "".to_string();
        #[cfg(feature = "flatpak")]
        let dir = "/app";
        #[cfg(not(feature = "flatpak"))]
        let dir = "/usr";
        sciter::set_library(&(prefix + dir + "/lib/rustdesk/libsciter-gtk.so")).ok();
    }
    #[cfg(windows)]
    // Check if there is a sciter.dll nearby.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sciter_dll_path = parent.join("sciter.dll");
            if sciter_dll_path.exists() {
                // Try to set the sciter dll.
                let p = sciter_dll_path.to_string_lossy().to_string();
                log::debug!("Found dll:{}, \n {:?}", p, sciter::set_library(&p));
            }
        }
    }
    // https://github.com/c-smile/sciter-sdk/blob/master/include/sciter-x-types.h
    // https://github.com/rustdesk/rustdesk/issues/132#issuecomment-886069737
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::GfxLayer(
        sciter::GFX_LAYER::WARP
    )));
    use sciter::SCRIPT_RUNTIME_FEATURES::*;
    allow_err!(sciter::set_options(sciter::RuntimeOptions::ScriptFeatures(
        ALLOW_FILE_IO as u8 | ALLOW_SOCKET_IO as u8 | ALLOW_EVAL as u8 | ALLOW_SYSINFO as u8
    )));
    let mut frame = sciter::WindowBuilder::main_window().create();
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::UxTheming(true)));
    frame.set_title(&crate::get_app_name());
    #[cfg(target_os = "macos")]
    crate::platform::delegate::make_menubar(frame.get_host(), args.is_empty());
    let page;
    if args.len() > 1 && args[0] == "--play" {
        args[0] = "--connect".to_owned();
        let path: std::path::PathBuf = (&args[1]).into();
        let id = path
            .file_stem()
            .map(|p| p.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned();
        args[1] = id;
    }
    if args.is_empty() {
        let children: Children = Default::default();
        std::thread::spawn(move || check_zombie(children));
        crate::common::check_software_update();
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "index.html";
        // Start pulse audio local server.
        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);
    } else if args[0] == "--install" {
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "install.html";
    } else if args[0] == "--cm" {
        frame.register_behavior("connection-manager", move || {
            Box::new(cm::SciterConnectionManager::new())
        });
        page = "cm.html";
    } else if (args[0] == "--connect"
        || args[0] == "--file-transfer"
        || args[0] == "--port-forward"
        || args[0] == "--rdp")
        && args.len() > 1
    {
        #[cfg(windows)]
        {
            let hw = frame.get_host().get_hwnd();
            crate::platform::windows::enable_lowlevel_keyboard(hw as _);
        }
        let mut iter = args.iter();
        let cmd = iter.next().unwrap().clone();
        let id = iter.next().unwrap().clone();
        let pass = iter.next().unwrap_or(&"".to_owned()).clone();
        let args: Vec<String> = iter.map(|x| x.clone()).collect();
        frame.set_title(&id);
        frame.register_behavior("native-remote", move || {
            let handler =
                remote::SciterSession::new(cmd.clone(), id.clone(), pass.clone(), args.clone());
            #[cfg(not(any(feature = "flutter", feature = "cli")))]
            {
                *CUR_SESSION.lock().unwrap() = Some(handler.inner());
            }
            Box::new(handler)
        });
        page = "remote.html";
    } else {
        log::error!("Wrong command: {:?}", args);
        return;
    }
    #[cfg(feature = "inline")]
    {
        let html = if page == "index.html" {
            inline::get_index()
        } else if page == "cm.html" {
            inline::get_cm()
        } else if page == "install.html" {
            inline::get_install()
        } else {
            inline::get_remote()
        };
        frame.load_html(html.as_bytes(), Some(page));
    }
    #[cfg(not(feature = "inline"))]
    frame.load_file(&format!(
        "file://{}/src/ui/{}",
        std::env::current_dir()
            .map(|c| c.display().to_string())
            .unwrap_or("".to_owned()),
        page
    ));
    frame.run_app();
}

struct UI {}

impl UI {
    fn recent_sessions_updated(&self) -> bool {
        recent_sessions_updated()
    }

    fn get_id(&self) -> String {
        ipc::get_id()
    }

    fn temporary_password(&mut self) -> String {
        temporary_password()
    }

    fn update_temporary_password(&self) {
        update_temporary_password()
    }

    fn permanent_password(&self) -> String {
        permanent_password()
    }

    fn set_permanent_password(&self, password: String) {
        set_permanent_password(password);
    }

    fn get_remote_id(&mut self) -> String {
        LocalConfig::get_remote_id()
    }

    fn set_remote_id(&mut self, id: String) {
        LocalConfig::set_remote_id(&id);
    }

    fn goto_install(&mut self) {
        goto_install();
    }

    fn install_me(&mut self, _options: String, _path: String) {
        install_me(_options, _path, false, false);
    }

    fn update_me(&self, _path: String) {
        update_me(_path);
    }

    fn run_without_install(&self) {
        run_without_install();
    }

    fn show_run_without_install(&self) -> bool {
        show_run_without_install()
    }

    fn get_license(&self) -> String {
        get_license()
    }

    fn get_option(&self, key: String) -> String {
        get_option(key)
    }

    fn get_local_option(&self, key: String) -> String {
        get_local_option(key)
    }

    fn set_local_option(&self, key: String, value: String) {
        set_local_option(key, value);
    }

    fn peer_has_password(&self, id: String) -> bool {
        peer_has_password(id)
    }

    fn forget_password(&self, id: String) {
        forget_password(id)
    }

    fn get_peer_option(&self, id: String, name: String) -> String {
        get_peer_option(id, name)
    }

    fn set_peer_option(&self, id: String, name: String, value: String) {
        set_peer_option(id, name, value)
    }

    fn using_public_server(&self) -> bool {
        using_public_server()
    }

    fn get_options(&self) -> Value {
        let hashmap: HashMap<String, String> = serde_json::from_str(&get_options()).unwrap();
        let mut m = Value::map();
        for (k, v) in hashmap {
            m.set_item(k, v);
        }
        m
    }

    fn test_if_valid_server(&self, host: String) -> String {
        test_if_valid_server(host)
    }

    fn get_sound_inputs(&self) -> Value {
        Value::from_iter(get_sound_inputs())
    }

    fn set_options(&self, v: Value) {
        let mut m = HashMap::new();
        for (k, v) in v.items() {
            if let Some(k) = k.as_string() {
                if let Some(v) = v.as_string() {
                    if !v.is_empty() {
                        m.insert(k, v);
                    }
                }
            }
        }
        set_options(m);
    }

    fn set_option(&self, key: String, value: String) {
        set_option(key, value);
    }

    fn install_path(&mut self) -> String {
        install_path()
    }

    fn get_socks(&self) -> Value {
        Value::from_iter(get_socks())
    }

    fn set_socks(&self, proxy: String, username: String, password: String) {
        set_socks(proxy, username, password)
    }

    fn is_installed(&self) -> bool {
        is_installed()
    }

    fn is_root(&self) -> bool {
        is_root()
    }

    fn is_release(&self) -> bool {
        #[cfg(not(debug_assertions))]
        return true;
        #[cfg(debug_assertions)]
        return false;
    }

    fn is_rdp_service_open(&self) -> bool {
        is_rdp_service_open()
    }

    fn is_share_rdp(&self) -> bool {
        is_share_rdp()
    }

    fn set_share_rdp(&self, _enable: bool) {
        set_share_rdp(_enable);
    }

    fn is_installed_lower_version(&self) -> bool {
        is_installed_lower_version()
    }

    fn closing(&mut self, x: i32, y: i32, w: i32, h: i32) {
        crate::server::input_service::fix_key_down_timeout_at_exit();
        LocalConfig::set_size(x, y, w, h);
    }

    fn get_size(&mut self) -> Value {
        let s = LocalConfig::get_size();
        let mut v = Vec::new();
        v.push(s.0);
        v.push(s.1);
        v.push(s.2);
        v.push(s.3);
        Value::from_iter(v)
    }

    fn get_mouse_time(&self) -> f64 {
        get_mouse_time()
    }

    fn check_mouse_time(&self) {
        check_mouse_time()
    }

    fn get_connect_status(&mut self) -> Value {
        let mut v = Value::array(0);
        let x = get_connect_status();
        v.push(x.status_num);
        v.push(x.key_confirmed);
        v.push(x.id);
        v
    }

    #[inline]
    fn get_peer_value(id: String, p: PeerConfig) -> Value {
        let values = vec![
            id,
            p.info.username.clone(),
            p.info.hostname.clone(),
            p.info.platform.clone(),
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ];
        Value::from_iter(values)
    }

    fn get_peer(&self, id: String) -> Value {
        let c = get_peer(id.clone());
        Self::get_peer_value(id, c)
    }

    fn get_fav(&self) -> Value {
        Value::from_iter(get_fav())
    }

    fn store_fav(&self, fav: Value) {
        let mut tmp = vec![];
        fav.values().for_each(|v| {
            if let Some(v) = v.as_string() {
                if !v.is_empty() {
                    tmp.push(v);
                }
            }
        });
        store_fav(tmp);
    }

    fn get_recent_sessions(&mut self) -> Value {
        // to-do: limit number of recent sessions, and remove old peer file
        let peers: Vec<Value> = PeerConfig::peers()
            .drain(..)
            .map(|p| Self::get_peer_value(p.0, p.2))
            .collect();
        Value::from_iter(peers)
    }

    fn get_icon(&mut self) -> String {
        get_icon()
    }

    fn remove_peer(&mut self, id: String) {
        PeerConfig::remove(&id);
    }

    fn remove_discovered(&mut self, id: String) {
        remove_discovered(id);
    }

    fn send_wol(&mut self, id: String) {
        crate::lan::send_wol(id)
    }

    fn new_remote(&mut self, id: String, remote_type: String, force_relay: bool) {
        new_remote(id, remote_type, force_relay)
    }

    fn is_process_trusted(&mut self, _prompt: bool) -> bool {
        is_process_trusted(_prompt)
    }

    fn is_can_screen_recording(&mut self, _prompt: bool) -> bool {
        is_can_screen_recording(_prompt)
    }

    fn is_installed_daemon(&mut self, _prompt: bool) -> bool {
        is_installed_daemon(_prompt)
    }

    fn get_error(&mut self) -> String {
        get_error()
    }

    fn is_login_wayland(&mut self) -> bool {
        is_login_wayland()
    }

    fn current_is_wayland(&mut self) -> bool {
        current_is_wayland()
    }

    fn get_software_update_url(&self) -> String {
        crate::SOFTWARE_UPDATE_URL.lock().unwrap().clone()
    }

    fn get_new_version(&self) -> String {
        get_new_version()
    }

    fn get_version(&self) -> String {
        get_version()
    }

    fn get_fingerprint(&self) -> String {
        get_fingerprint()
    }

    fn get_app_name(&self) -> String {
        get_app_name()
    }

    fn get_software_ext(&self) -> String {
        #[cfg(windows)]
        let p = "exe";
        #[cfg(target_os = "macos")]
        let p = "dmg";
        #[cfg(target_os = "linux")]
        let p = "deb";
        p.to_owned()
    }

    fn get_software_store_path(&self) -> String {
        let mut p = std::env::temp_dir();
        let name = crate::SOFTWARE_UPDATE_URL
            .lock()
            .unwrap()
            .split("/")
            .last()
            .map(|x| x.to_owned())
            .unwrap_or(crate::get_app_name());
        p.push(name);
        format!("{}.{}", p.to_string_lossy(), self.get_software_ext())
    }

    fn create_shortcut(&self, _id: String) {
        #[cfg(windows)]
        create_shortcut(_id)
    }

    fn discover(&self) {
        std::thread::spawn(move || {
            allow_err!(crate::lan::discover());
        });
    }

    fn get_lan_peers(&self) -> String {
        // let peers = get_lan_peers()
        //     .into_iter()
        //     .map(|mut peer| {
        //         (
        //             peer.remove("id").unwrap_or_default(),
        //             peer.remove("username").unwrap_or_default(),
        //             peer.remove("hostname").unwrap_or_default(),
        //             peer.remove("platform").unwrap_or_default(),
        //         )
        //     })
        //     .collect::<Vec<(String, String, String, String)>>();
        serde_json::to_string(&get_lan_peers()).unwrap_or_default()
    }

    fn get_uuid(&self) -> String {
        get_uuid()
    }

    fn open_url(&self, url: String) {
        #[cfg(windows)]
        let p = "explorer";
        #[cfg(target_os = "macos")]
        let p = "open";
        #[cfg(target_os = "linux")]
        let p = if std::path::Path::new("/usr/bin/firefox").exists() {
            "firefox"
        } else {
            "xdg-open"
        };
        allow_err!(std::process::Command::new(p).arg(url).spawn());
    }

    fn change_id(&self, id: String) {
        reset_async_job_status();
        let old_id = self.get_id();
        change_id_shared(id, old_id);
    }

    fn post_request(&self, url: String, body: String, header: String) {
        post_request(url, body, header)
    }

    fn is_ok_change_id(&self) -> bool {
        hbb_common::machine_uid::get().is_ok()
    }

    fn get_async_job_status(&self) -> String {
        get_async_job_status()
    }

    fn t(&self, name: String) -> String {
        crate::client::translate(name)
    }

    fn is_xfce(&self) -> bool {
        crate::platform::is_xfce()
    }

    fn get_api_server(&self) -> String {
        get_api_server()
    }

    fn has_hwcodec(&self) -> bool {
        has_hwcodec()
    }

    fn get_langs(&self) -> String {
        get_langs()
    }

    fn default_video_save_directory(&self) -> String {
        default_video_save_directory()
    }

    fn handle_relay_id(&self, id: String) -> String {
        handle_relay_id(id)
    }

    fn get_hostname(&self) -> String {
        get_hostname()
    }
}

impl sciter::EventHandler for UI {
    sciter::dispatch_script_call! {
        fn t(String);
        fn get_api_server();
        fn is_xfce();
        fn using_public_server();
        fn get_id();
        fn temporary_password();
        fn update_temporary_password();
        fn permanent_password();
        fn set_permanent_password(String);
        fn get_remote_id();
        fn set_remote_id(String);
        fn closing(i32, i32, i32, i32);
        fn get_size();
        fn new_remote(String, String, bool);
        fn send_wol(String);
        fn remove_peer(String);
        fn remove_discovered(String);
        fn get_connect_status();
        fn get_mouse_time();
        fn check_mouse_time();
        fn get_recent_sessions();
        fn get_peer(String);
        fn get_fav();
        fn store_fav(Value);
        fn recent_sessions_updated();
        fn get_icon();
        fn install_me(String, String);
        fn is_installed();
        fn is_root();
        fn is_release();
        fn set_socks(String, String, String);
        fn get_socks();
        fn is_rdp_service_open();
        fn is_share_rdp();
        fn set_share_rdp(bool);
        fn is_installed_lower_version();
        fn install_path();
        fn goto_install();
        fn is_process_trusted(bool);
        fn is_can_screen_recording(bool);
        fn is_installed_daemon(bool);
        fn get_error();
        fn is_login_wayland();
        fn current_is_wayland();
        fn get_options();
        fn get_option(String);
        fn get_local_option(String);
        fn set_local_option(String, String);
        fn get_peer_option(String, String);
        fn peer_has_password(String);
        fn forget_password(String);
        fn set_peer_option(String, String, String);
        fn get_license();
        fn test_if_valid_server(String);
        fn get_sound_inputs();
        fn set_options(Value);
        fn set_option(String, String);
        fn get_software_update_url();
        fn get_new_version();
        fn get_version();
        fn get_fingerprint();
        fn update_me(String);
        fn show_run_without_install();
        fn run_without_install();
        fn get_app_name();
        fn get_software_store_path();
        fn get_software_ext();
        fn open_url(String);
        fn change_id(String);
        fn get_async_job_status();
        fn post_request(String, String, String);
        fn is_ok_change_id();
        fn create_shortcut(String);
        fn discover();
        fn get_lan_peers();
        fn get_uuid();
        fn has_hwcodec();
        fn get_langs();
        fn default_video_save_directory();
        fn handle_relay_id(String);
        fn get_hostname();
    }
}

impl sciter::host::HostHandler for UIHostHandler {
    fn on_graphics_critical_failure(&mut self) {
        log::error!("Critical rendering error: e.g. DirectX gfx driver error. Most probably bad gfx drivers.");
    }
}

pub fn check_zombie(children: Children) {
    let mut deads = Vec::new();
    loop {
        let mut lock = children.lock().unwrap();
        let mut n = 0;
        for (id, c) in lock.1.iter_mut() {
            if let Ok(Some(_)) = c.try_wait() {
                deads.push(id.clone());
                n += 1;
            }
        }
        for ref id in deads.drain(..) {
            lock.1.remove(id);
        }
        if n > 0 {
            lock.0 = true;
        }
        drop(lock);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "linux"))]
fn get_sound_inputs() -> Vec<String> {
    let mut out = Vec::new();
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Ok(devices) = host.devices() {
        for device in devices {
            if device.default_input_config().is_err() {
                continue;
            }
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn get_sound_inputs() -> Vec<String> {
    crate::platform::linux::get_pa_sources()
        .drain(..)
        .map(|x| x.1)
        .collect()
}

// sacrifice some memory
pub fn value_crash_workaround(values: &[Value]) -> Arc<Vec<Value>> {
    let persist = Arc::new(values.to_vec());
    STUPID_VALUES.lock().unwrap().push(persist.clone());
    persist
}

#[inline]
pub fn new_remote(id: String, remote_type: String, force_relay: bool) {
    let mut lock = CHILDREN.lock().unwrap();
    let mut args = vec![format!("--{}", remote_type), id.clone()];
    if force_relay {
        args.push("".to_string()); // password
        args.push("--relay".to_string());
    }
    let key = (id.clone(), remote_type.clone());
    if let Some(c) = lock.1.get_mut(&key) {
        if let Ok(Some(_)) = c.try_wait() {
            lock.1.remove(&key);
        } else {
            if remote_type == "rdp" {
                allow_err!(c.kill());
                std::thread::sleep(std::time::Duration::from_millis(30));
                c.try_wait().ok();
                lock.1.remove(&key);
            } else {
                return;
            }
        }
    }
    match crate::run_me(args) {
        Ok(child) => {
            lock.1.insert(key, child);
        }
        Err(err) => {
            log::error!("Failed to spawn remote: {}", err);
        }
    }
}

#[inline]
pub fn recent_sessions_updated() -> bool {
    let mut children = CHILDREN.lock().unwrap();
    if children.0 {
        children.0 = false;
        true
    } else {
        false
    }
}

pub fn get_icon() -> String {
    "data:image/png;base64,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".into()
}
