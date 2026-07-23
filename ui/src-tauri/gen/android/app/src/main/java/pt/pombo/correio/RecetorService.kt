package pt.pombo.correio

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder

/**
 * Foreground service que mantém a app a receber em segundo plano.
 *
 * Sem ele, o OxygenOS descarta as ligações de entrada assim que a app deixa de
 * estar em primeiro plano (o SYN chega e é filtrado), e a receção com o ecrã
 * desligado não funciona. Não hospeda trabalho: o servidor de receção corre no
 * processo da app (arrancado no `setup` do Rust); este serviço só mantém o
 * processo vivo e com prioridade de rede, e segura o MulticastLock para o mDNS.
 *
 * `START_NOT_STICKY` de propósito: se o SO matar o serviço, não o reinicia num
 * processo sem atividade — que ficaria sem o servidor Rust. Um foreground
 * service raramente é morto; se for, a pessoa reabre a app.
 */
class RecetorService : Service() {
    private var multicast: WifiManager.MulticastLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // MulticastLock: sem ele o Android filtra os pacotes multicast e o mDNS
        // não vê nem é visto na rede local. Peça B do plano, aqui porque é onde
        // já há contexto Android e o ciclo de vida é o certo.
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicast = wifi.createMulticastLock("pombo-mdns").apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (_: Throwable) {
            // Sem multicast, a descoberta cai para IP direto — não é fatal.
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val canal = "pombo_rececao"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (nm.getNotificationChannel(canal) == null) {
                nm.createNotificationChannel(
                    NotificationChannel(canal, "Receção", NotificationManager.IMPORTANCE_LOW)
                )
            }
        }

        val notificacao: Notification = Notification.Builder(this, canal)
            .setContentTitle("Pombo-Correio")
            .setContentText("Pronto a receber ficheiros")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setOngoing(true)
            .build()

        // O tipo tem de bater com o declarado no manifesto (specialUse, API 34+).
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(1, notificacao, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(1, notificacao, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(1, notificacao)
        }

        return START_NOT_STICKY
    }

    override fun onDestroy() {
        multicast?.let { if (it.isHeld) it.release() }
        multicast = null
        super.onDestroy()
    }
}
