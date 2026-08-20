import QtQuick
import org.kde.plasma.configuration

ConfigModel {
    ConfigCategory {
        name: i18n("Providers")
        icon: "cloud-download"
        source: "ConfigProviders.qml"
    }
    ConfigCategory {
        name: i18n("Appearance")
        icon: "preferences-desktop-color"
        source: "ConfigAppearance.qml"
    }
}
