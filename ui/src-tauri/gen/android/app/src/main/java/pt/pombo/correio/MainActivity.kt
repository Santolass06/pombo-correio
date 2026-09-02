package pt.pombo.correio

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.content.IntentCompat
import com.google.firebase.messaging.FirebaseMessaging

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Antes do super.onCreate (que arranca o Rust), para o ficheiro já lá estar
    // quando a interface montar e o for pedir.
    guardarPartilha(intent)

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

    // Q5: pede o token FCM a cada arranque, não só no `onNewToken` do
    // `FcmService` (que só dispara quando o token muda). Assíncrono — o Rust
    // vai espreitando o ficheiro no arranque, não há aqui nada a esperar.
    FirebaseMessaging.getInstance().token.addOnCompleteListener { tarefa ->
      if (tarefa.isSuccessful) {
        tarefa.result?.let { FcmService.escreverToken(this, it) }
      }
    }
  }

  // A app é `singleTask`: já estando aberta, uma partilha não repete o onCreate,
  // chega por aqui. Corre antes do onResume, por isso o ficheiro está escrito
  // quando a WebView reaparece e pergunta.
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    guardarPartilha(intent)
  }

  /**
   * Deixa em ficheiro os URIs que outra app partilhou para cá, um por linha.
   *
   * O ficheiro descreve **sempre** o intent atual — um intent que não é partilha
   * apaga-o. A permissão de leitura de um `content://` de partilha vale só
   * enquanto esta atividade viver e não é persistível; um URI sobrevivente de
   * uma partilha nunca consumida seria usado numa sessão em que já não vale, e
   * falharia com um erro de permissão que ninguém saberia ler.
   */
  private fun guardarPartilha(intent: Intent?) {
    val uris: List<Uri> = when (intent?.action) {
      Intent.ACTION_SEND ->
        listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
      Intent.ACTION_SEND_MULTIPLE ->
        IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
          ?: emptyList()
      // Inclui a partilha de texto simples, que não traz EXTRA_STREAM: não há
      // ficheiro nenhum para enviar.
      else -> emptyList()
    }

    try {
      val ficheiro = java.io.File(applicationInfo.dataDir, "partilha-pendente")
      if (uris.isEmpty()) ficheiro.delete()
      else ficheiro.writeText(uris.joinToString("\n"))
    } catch (_: Throwable) {
    }
  }
}
