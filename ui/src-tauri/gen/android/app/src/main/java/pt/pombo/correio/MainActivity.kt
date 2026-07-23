package pt.pombo.correio

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
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
