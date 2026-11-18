//! Se corre antes de compilar Orion: mete el ícono y los datos (nombre, versión, descripción)
//! dentro del .exe, como "recursos" de Windows. Es lo que se ve en el Explorador y en Propiedades → Detalles.
//! La versión y la descripción salen de Cargo.toml.

fn main() {
    println!("cargo:rerun-if-changed=assets/orion.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/orion.ico")
        .set("ProductName", "Orion")
        .set("FileDescription", "Orion")
        .set("LegalCopyright", "© 2026 1Bnja");
    if let Err(e) = res.compile() {
        // Sin el compilador de recursos (rc.exe, del Windows SDK) Orion igual compila, solo que sin ícono.
        println!("cargo:warning=no pude agregar el ícono al .exe: {e}");
    }
}
