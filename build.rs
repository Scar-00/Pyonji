use winresource::WindowsResource;

fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = WindowsResource::new();

        res.set_icon("./resources/icon.ico");
        res.set("FileDescription", "Pyonji");
        res.set("ProductName", "Pyonji");

        if let Err(e) = res.compile() {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
