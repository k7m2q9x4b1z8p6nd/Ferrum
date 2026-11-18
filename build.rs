fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("ProductName", "Ferrum");
        res.set("FileDescription", "Ferrum");
        res.compile().expect("winres");
    }
}
