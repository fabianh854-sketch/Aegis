// Bettet das App-Icon als Windows-Ressource in die .exe ein.
fn main() {
    println!("cargo:rerun-if-changed=aegis.rc");
    println!("cargo:rerun-if-changed=assets/icon/aegis.ico");
    // Nur beim Bauen für Windows; Tests auf anderen Systemen brauchen kein Ressourcen-Compiler.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("aegis.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("App-Icon konnte nicht eingebettet werden (Ressourcen-Compiler rc.exe fehlt?)");
    }
}
