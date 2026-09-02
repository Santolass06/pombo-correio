package pt.pombo.correio

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
 * `onMessageReceived` fica vazio de propósito: sem carga (`data: {}`, ver
 * `relay/src/fcm.rs`), o aviso em si não traz nada para mostrar. Quem acorda
 * o processo é o próprio sistema Android ao entregar a mensagem; a app,
 * uma vez viva, seria o `RecetorService`/o arranque normal a tratar do
 * resto — não há aqui nada a fazer.
 */
class FcmService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        escreverToken(this, token)
    }

    override fun onMessageReceived(mensagem: RemoteMessage) {
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
