cmake_minimum_required(VERSION 3.24)
project(telamon-gates VERSION 1.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
    set(CMAKE_INSTALL_PREFIX /usr CACHE PATH "" FORCE)
endif()
include(GNUInstallDirs)
find_package(Qt6 6.11 REQUIRED COMPONENTS Core Gui Qml Quick QuickControls2 Widgets)
qt_policy(SET QTP0001 NEW)
qt_policy(SET QTP0004 NEW)
# telamon-framework-ui's startup code (single instance, window activation)
find_package(KF6DBusAddons REQUIRED)
find_package(KF6WindowSystem REQUIRED)
find_package(Corrosion REQUIRED)
# Hugging Face downloads use the system's TLS (gates-core, through ureq).
find_package(OpenSSL REQUIRED)

set(WORKSPACE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../..)

# Rust library (CXX-Qt QObjects, telamon-framework, gates-core). cxx-qt-build
# finds Qt via $QMAKE.
find_program(QMAKE6 NAMES qmake6 qmake REQUIRED)
corrosion_import_crate(MANIFEST_PATH ${WORKSPACE_DIR}/Cargo.toml
    CRATES telamon-gates LOCKED)
corrosion_set_env_vars(telamon_gates QMAKE=${QMAKE6})

# Telamon.Ui, the shared Telamon look, is installed like Kirigami (telamon-ui):
# QML imports it and nothing links it. qmlcachegen and qmllint read its types
# from the installed copy.
if(NOT EXISTS "${QT6_INSTALL_PREFIX}/${QT6_INSTALL_QML}/Telamon/Ui/qmldir")
    message(FATAL_ERROR "Telamon.Ui is not installed: install telamon-ui (Telamon framework)")
endif()

qt_add_executable(telamon-gates-app cpp/main.cpp)
set_target_properties(telamon-gates-app PROPERTIES OUTPUT_NAME telamon-gates)

# qmlcachegen compiles every QML file to C++ at build time; the app reads no
# .qml from disk at run time.
qt_add_qml_module(telamon-gates-app
    URI net.eterneon.telamon.gates
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/ChatView.qml
        qml/MessageDelegate.qml
        qml/Composer.qml
        qml/SettingsPage.qml
        qml/AboutPage.qml
        qml/ModelsPage.qml
        qml/AgentPanel.qml
        qml/FleetPage.qml
)

target_link_libraries(telamon-gates-app PRIVATE
    telamon_gates
    Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick Qt6::QuickControls2 Qt6::Widgets
    KF6::DBusAddons KF6::WindowSystem
    OpenSSL::SSL OpenSSL::Crypto)

install(TARGETS telamon-gates-app RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES data/net.eterneon.telamon.gates.desktop
        DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
install(FILES data/net.eterneon.telamon.gates.svg
        DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps)
install(FILES data/net.eterneon.telamon.gates.metainfo.xml
        DESTINATION ${CMAKE_INSTALL_DATADIR}/metainfo)
