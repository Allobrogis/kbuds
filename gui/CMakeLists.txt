# SPDX-License-Identifier: GPL-3.0-or-later
cmake_minimum_required(VERSION 3.21)

project(kbuds
    VERSION 0.1.0
    DESCRIPTION "Pixel Buds settings (kbuds GUI)"
    LANGUAGES CXX
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui DBus Qml Quick QuickControls2)

qt_standard_project_setup(REQUIRES 6.8)

# The D-Bus interface XML is shared with the Rust daemon and CLI.
set(KBUDS_DBUS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../dbus"
    CACHE PATH "Directory holding the io.github.kbuds.*.xml interface files")

set(manager_xml "${KBUDS_DBUS_DIR}/io.github.kbuds.Manager1.xml")
set(device_xml "${KBUDS_DBUS_DIR}/io.github.kbuds.Device1.xml")
set_source_files_properties(${manager_xml} PROPERTIES
    CLASSNAME KbudsManagerProxy
    NO_NAMESPACE ON
)
set_source_files_properties(${device_xml} PROPERTIES
    CLASSNAME KbudsDeviceProxy
    NO_NAMESPACE ON
)
qt_add_dbus_interface(dbus_sources ${manager_xml} manager1_interface)
qt_add_dbus_interface(dbus_sources ${device_xml} device1_interface)

qt_add_executable(kbuds
    src/main.cpp
    src/daemonclient.cpp
    src/daemonclient.h
    src/device.cpp
    src/device.h
    ${dbus_sources}
)

qt_add_qml_module(kbuds
    URI Kbuds
    QML_FILES
        Main.qml
        AudioPanel.qml
        AudioProblemBanner.qml
        BatteryCard.qml
        ConnectionBanner.qml
        ConnectionStatus.qml
        DevicePage.qml
        EmptyState.qml
        ErrorPopup.qml
        NoiseControl.qml
        SettingSwitch.qml
    RESOURCES
        icons/io.github.kbuds.svg
)

qt_add_resources(kbuds "controls_config"
    PREFIX "/"
    FILES
        qtquickcontrols2.conf
)

target_include_directories(kbuds PRIVATE
    src
    # qdbusxml2cpp output
    ${CMAKE_CURRENT_BINARY_DIR}
)

target_compile_definitions(kbuds PRIVATE
    KBUDS_VERSION="${PROJECT_VERSION}"
    QT_NO_CAST_FROM_ASCII
)

target_link_libraries(kbuds PRIVATE
    Qt6::DBus
    Qt6::Quick
    Qt6::QuickControls2
)

install(TARGETS kbuds
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)
install(FILES io.github.kbuds.desktop
    DESTINATION ${CMAKE_INSTALL_DATADIR}/applications
)
install(FILES icons/io.github.kbuds.svg
    DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps
)
