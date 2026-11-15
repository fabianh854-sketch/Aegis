// Bettet das App-Icon als Windows-Ressource in aegis-updater.exe ein.
fn main() {
    println!("cargo:rerun-if-changed=aegis-updater.rc");
    println!("cargo:rerun-if-changed=../aegis-app/assets/icon/aegis.ico");
    // Nur beim Bauen für Windows; Tests auf anderen Systemen brauchen keinen Ressourcen-Compiler.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("aegis-updater.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("Icon konnte nicht eingebettet werden (Ressourcen-Compiler rc.exe fehlt?)");
    }
}
