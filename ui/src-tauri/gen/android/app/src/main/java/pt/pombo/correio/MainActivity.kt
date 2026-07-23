package pt.pombo.correio

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // O nome que a pessoa reconhece é o das Definições ("Nome do dispositivo",
    // ex. "OnePlus Pad 2"), só acessível por código Android. Escreve-se num
    // ficheiro ANTES do super.onCreate (que arranca o Rust), para o Rust o poder
    // ler ao criar/atualizar a identidade.
    try {
      val nome = android.provider.Settings.Global.getString(contentResolver, "device_name")
      if (!nome.isNullOrBlank()) {
        java.io.File(applicationInfo.dataDir, "nome-dispositivo").writeText(nome.trim())
      }
    } catch (_: Throwable) {
    }

    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // Arranca o foreground service que mantém a app a receber em segundo plano
    // (sem ele o OxygenOS bloqueia as ligações de entrada) e segura o
    // MulticastLock do mDNS. Pára sozinho quando a app é retirada (stopWithTask).
    val servico = Intent(this, RecetorService::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      startForegroundService(servico)
    } else {
      startService(servico)
    }
  }
}
