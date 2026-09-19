cmake_minimum_required(VERSION 3.15)
project(IPMsgPro VERSION 1.5.0 LANGUAGES CXX C)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(MSVC)
    add_compile_options(/utf-8 /MP)

    # Static linking of CRT - no runtime DLL dependencies
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
    # Ensure static CRT for all targets
    add_compile_options(
        $<$<CONFIG:Release>:/MT>
        $<$<CONFIG:Debug>:/MTd>
    )

    # DPI awareness manifest
    set(DPI_MANIFEST_XML "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>
<assembly xmlns='urn:schemas-microsoft-com:asm.v1' manifestVersion='1.0'>
  <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level='asInvoker' uiAccess='false' />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application>
    <windowsSettings>
      <dpiAware xmlns='http://schemas.microsoft.com/SMI/2005/WindowsSettings'>true/pm</dpiAware>
      <dpiAwareness xmlns='http://schemas.microsoft.com/SMI/2016/WindowsSettings'>PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns='urn:schemas-microsoft-com:compatibility.v1'>
    <application>
      <supportedOS Id='{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}'/>
      <supportedOS Id='{1f676c76-80e1-4239-95bb-83d0f6d0da78}'/>
      <supportedOS Id='{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}'/>
      <supportedOS Id='{35138b9a-5d96-4fbd-8e2d-a2440225f93a}'/>
    </application>
  </compatibility>
</assembly>")
    file(WRITE "${CMAKE_BINARY_DIR}/generated/dpi_awareness.manifest" "${DPI_MANIFEST_XML}")
endif()

# ============================================================================
# Dependencies
# ============================================================================

# TauriCPP framework
set(TAURICPP_DIR "${CMAKE_SOURCE_DIR}/TauriCPP")

# nlohmann/json
set(NLOHMANN_JSON_DIR "${TAURICPP_DIR}/third_party/nlohmann")

# Determine architecture from CMake generator (x64 or Win32/x86)
if(CMAKE_SIZEOF_VOID_P EQUAL 8)
    set(BUILD_ARCH "x64")
    set(WEBVIEW2_ARCH_PATH "x64-windows")
else()
    set(BUILD_ARCH "x86")
    set(WEBVIEW2_ARCH_PATH "x86-windows")
endif()

message(STATUS "Building for architecture: ${BUILD_ARCH}")

# WebView2 SDK - search paths vary by architecture
# For third_party, include arch-specific subdirectory (e.g., lib/x64/, lib/x86/)
set(WEBVIEW2_SEARCH_PATHS
    "${TAURICPP_DIR}/vcpkg/installed/${WEBVIEW2_ARCH_PATH}"
    "$ENV{VCPKG_ROOT}/installed/${WEBVIEW2_ARCH_PATH}"
    "C:/vcpkg/installed/${WEBVIEW2_ARCH_PATH}"
    "${CMAKE_SOURCE_DIR}/third_party/WebView2/${BUILD_ARCH}"
    "${CMAKE_SOURCE_DIR}/third_party/WebView2"
)

find_path(WEBVIEW2_INCLUDE_DIR
    NAMES WebView2.h
    PATH_SUFFIXES include
    PATHS ${WEBVIEW2_SEARCH_PATHS}
)

find_library(WEBVIEW2_LIBRARY
    NAMES WebView2LoaderStatic WebView2Loader.dll.lib
    PATH_SUFFIXES lib
    PATHS ${WEBVIEW2_SEARCH_PATHS}
)
if(NOT WEBVIEW2_INCLUDE_DIR)
    message(FATAL_ERROR
        "WebView2 SDK not found!\n"
        "Install via: vcpkg install webview2 --classic\n"
        "Or download from: https://www.nuget.org/packages/Microsoft.Web.WebView2"
    )
endif()

message(STATUS "WebView2 include: ${WEBVIEW2_INCLUDE_DIR}")
message(STATUS "WebView2 library: ${WEBVIEW2_LIBRARY}")

# ============================================================================
# TauriCPP Static Library (per-architecture)
# ============================================================================
set(TAURICPP_TARGET "tauricpp_${BUILD_ARCH}")

add_library(${TAURICPP_TARGET} STATIC
    ${TAURICPP_DIR}/src/app.cpp
    ${TAURICPP_DIR}/src/bridge.cpp
    ${TAURICPP_DIR}/src/clipboard.cpp
    ${TAURICPP_DIR}/src/dialog.cpp
    ${TAURICPP_DIR}/src/virtual_fs.cpp
    ${TAURICPP_DIR}/src/window.cpp
    ${TAURICPP_DIR}/src/embedded_dll.cpp
)

target_include_directories(${TAURICPP_TARGET} PUBLIC
    ${TAURICPP_DIR}/include
    ${NLOHMANN_JSON_DIR}/..
    ${WEBVIEW2_INCLUDE_DIR}
)

target_link_libraries(${TAURICPP_TARGET} PUBLIC
    shlwapi
    shell32
    comdlg32
    ole32
    ${WEBVIEW2_LIBRARY}
)

# ============================================================================
# SQLite3 (per-architecture)
# ============================================================================
set(SQLITE3_TARGET "sqlite3_${BUILD_ARCH}")
set(SQLITE3_DIR "${CMAKE_SOURCE_DIR}/src/external/sqlite3")

add_library(${SQLITE3_TARGET} STATIC
    ${SQLITE3_DIR}/sqlite3.c
)

target_include_directories(${SQLITE3_TARGET} PUBLIC
    ${SQLITE3_DIR}
)

# SQLite3 needs threading support
if(MSVC)
    target_compile_definitions(${SQLITE3_TARGET} PRIVATE
        SQLITE_THREADSAFE=1
        SQLITE_OMIT_LOAD_EXTENSION
    )
endif()

# ============================================================================
# Resource packing for frontend
# ============================================================================
set(FRONTEND_DIR "${CMAKE_SOURCE_DIR}/frontend/dist")
set(GEN_DIR "${CMAKE_BINARY_DIR}/generated")
file(MAKE_DIRECTORY "${GEN_DIR}")
set(RESOURCES_RC "${GEN_DIR}/resources.rc")
set(RESOURCE_MAP_CPP "${GEN_DIR}/resource_map.cpp")

find_package(Python3 COMPONENTS Interpreter)

if(Python3_Interpreter_FOUND)
    file(GLOB_RECURSE FRONTEND_FILES "${FRONTEND_DIR}/*")
    add_custom_command(
        OUTPUT "${RESOURCES_RC}" "${RESOURCE_MAP_CPP}"
        COMMAND ${Python3_EXECUTABLE} "${TAURICPP_DIR}/tools/pack_resources.py"
                "${FRONTEND_DIR}" -o "${RESOURCES_RC}" -t "TAURI_RES"
        DEPENDS "${TAURICPP_DIR}/tools/pack_resources.py" ${FRONTEND_FILES}
        COMMENT "Packing frontend resources into .rc file"
        VERBATIM
    )
else()
    file(WRITE "${RESOURCES_RC}" "// Auto-generated resources\n")
    file(WRITE "${RESOURCE_MAP_CPP}" "// Auto-generated\n")
endif()

# ============================================================================
# IPMsgPro Application
# ============================================================================
set(IPMSGPRO_SOURCES
    src/main.cpp
    src/cli_runner.cpp
    src/logger.cpp
    src/util/encoding.cpp
    src/util/app_paths.cpp
    src/util/base64.cpp
    src/ipmsg/msgmng.cpp
    src/ipmsg/network.cpp
    src/bridge/command_handler.cpp
    src/bridge/notification_sound.cpp
    src/bridge/screen_capture.cpp
    src/bridge/feiq_screenshot.cpp
    src/database/message_db.cpp
    src/file/file_transfer.cpp
)

set(IPMSGPRO_ADDITIONAL_SOURCES "")
if(MSVC)
    list(APPEND IPMSGPRO_ADDITIONAL_SOURCES "${CMAKE_BINARY_DIR}/generated/dpi_awareness.manifest")
    list(APPEND IPMSGPRO_ADDITIONAL_SOURCES "${CMAKE_SOURCE_DIR}/resources/app.rc")
endif()

# Determine output name based on architecture
if(BUILD_ARCH STREQUAL "x64")
    set(IPMSGPRO_OUTPUT_NAME "SpeedIpMsg")
else()
    set(IPMSGPRO_OUTPUT_NAME "SpeedIpMsg_X86")
endif()

message(STATUS "SpeedIpMsg output name: ${IPMSGPRO_OUTPUT_NAME}")

add_executable(SpeedIpMsg WIN32
    ${IPMSGPRO_SOURCES}
    "${RESOURCE_MAP_CPP}"
    "${RESOURCES_RC}"
    ${IPMSGPRO_ADDITIONAL_SOURCES}
)

# Ensure output name matches architecture
set_target_properties(SpeedIpMsg PROPERTIES OUTPUT_NAME "${IPMSGPRO_OUTPUT_NAME}")

target_include_directories(SpeedIpMsg PRIVATE
    "${CMAKE_SOURCE_DIR}/src"
    "${GEN_DIR}"
    ${TAURICPP_DIR}/include
    ${NLOHMANN_JSON_DIR}/..
    ${SQLITE3_DIR}
    ${WEBVIEW2_INCLUDE_DIR}
)

target_link_libraries(SpeedIpMsg PRIVATE
    ${TAURICPP_TARGET}
    ${SQLITE3_TARGET}
    ws2_32
    iphlpapi
    winmm
    gdiplus
)

# Force static CRT linking for the executable
if(MSVC)
    set_target_properties(SpeedIpMsg PROPERTIES
        MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>"
    )
    # Static link all dependencies
    target_link_options(SpeedIpMsg PRIVATE
        $<$<CONFIG:Release>:/NODEFAULTLIB:MSVCRT>
        $<$<CONFIG:Release>:/NODEFAULTLIB:MSVCRTD>
    )
endif()

add_dependencies(SpeedIpMsg pack_resources)

add_custom_target(pack_resources
    DEPENDS "${RESOURCES_RC}" "${RESOURCE_MAP_CPP}"
    COMMENT "Pack frontend resources"
)

