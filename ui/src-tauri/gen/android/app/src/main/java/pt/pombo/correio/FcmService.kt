package pt.pombo.correio

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import java.io.File

/**
 * Só recebe o token FCM e escreve-o em ficheiro — mesmo padrão do
 * `nome-dispositivo` e da `partilha-pendente`: o Rust não fala Kotlin, por
 * isso a ponte é sempre um ficheiro que ele lê ao arrancar ou vai
 * espreitando.
 *
 * `onNewToken` só dispara quando o token muda (raro: reinstalação, app a
 * limpar dados, rotação da própria Google). O pedido normal, a cada arranque
 * da app, é feito pela `MainActivity` — este serviço cobre só a mudança fora
 * desse ciclo.
 *
 * `onMessageReceived` (Review3 F-12): sem isto, o processo acorda — é o
 * próprio contrato do FCM — mas ninguém vê nada. O sistema **não** arranca
 * a `MainActivity` nem o `RecetorService` sozinho, só entrega a mensagem
 * aqui; um `onMessageReceived` vazio deixa o processo morrer logo a seguir
 * sem rasto nenhum. Por isso a notificação local é feita aqui mesmo. Sem
 * carga (`data: {}`, ver `relay/src/fcm.rs` — o relay não sabe o que
 * chegou), por isso o texto é sempre o mesmo, genérico; tocar-lhe é que
 * abre a app, que já sabe perguntar ao relay o que é.
 */
class FcmService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        escreverToken(this, token)
    }

    override fun onMessageReceived(mensagem: RemoteMessage) {
        val canal = "pombo_push"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (nm.getNotificationChannel(canal) == null) {
                nm.createNotificationChannel(
                    NotificationChannel(canal, "Ficheiros novos", NotificationManager.IMPORTANCE_HIGH)
                )
            }
        }

        val abrir = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val pendente = PendingIntent.getActivity(
            this, 0, abrir, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val notificacao = NotificationCompat.Builder(this, canal)
            .setContentTitle("Pombo-Correio")
            .setContentText("Chegou um ficheiro novo")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentIntent(pendente)
            .setAutoCancel(true)
            .build()

        // Um único id: um segundo aviso antes de o primeiro ser aberto
        // substitui-o em vez de empilhar — a mesma escolha da
        // `partilha-pendente`, não há aqui uma lista do que chegou.
        NotificationManagerCompat.from(this).notify(2, notificacao)
    }

    companion object {
        fun escreverToken(contexto: android.content.Context, token: String) {
            try {
                File(contexto.applicationInfo.dataDir, "fcm-token").writeText(token)
            } catch (_: Throwable) {
            }
        }
    }
}
