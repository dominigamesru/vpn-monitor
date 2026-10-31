//! Если при сборке есть `assets/favicon.ico` (CI качает фавикон dominigames.com), он становится логотипом в
//! трее (`cfg(has_favicon)`) и иконкой exe. Иначе логотип рисуется в коде.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_favicon)");
    println!("cargo:rerun-if-changed=assets/favicon.ico");
    let fav = std::path::Path::new("assets/favicon.ico");
    let has = fav.exists();
    if has {
        println!("cargo:rustc-cfg=has_favicon");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        if has {
            res.set_icon("assets/favicon.ico");
        }
        res.set("ProductName", "Domini VPN Monitor");
        res.set("FileDescription", "Domini VPN Monitor");
        res.set("CompanyName", "Domini Games");
        if let Err(e) = res.compile() {
            println!("cargo:warning=ресурсы exe не встроены: {e}");
        }
    }
}
