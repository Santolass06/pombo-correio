// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // O renderizador DMABUF do WebKitGTK não funciona com o GPU do Raspberry Pi
    // (VideoCore VII) e a janela sai coberta de manchas brancas e pretas. Desligá-lo
    // volta a compor por software, o que num Pi é perfeitamente aceitável — a
    // interface é estática. Só se toca no ARM Linux, para não penalizar o x86-64,
    // que rende bem pelo caminho acelerado. Respeita-se um valor já definido, para
    // quem queira forçar o contrário.
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: início do main, antes de haver outras threads — nenhum outro
        // código pode estar a ler o ambiente ao mesmo tempo.
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    pombo_app_lib::run();
}
