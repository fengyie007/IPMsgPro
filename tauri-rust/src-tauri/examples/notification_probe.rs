//! Native diagnostic for an already registered application identity. Does not
//! change shortcuts or registry. --send-test posts a silent, suppressed toast
//! and removes only its unique tag. Run under the interactive desktop account.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use windows::{
        core::HSTRING,
        Data::Xml::Dom::XmlDocument,
        Foundation::TypedEventHandler,
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
        UI::Notifications::{ToastFailedEventArgs, ToastNotification, ToastNotificationManager},
    };
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    let _apartment = Apartment;
    let app_id = HSTRING::from("com.speedipmsg.rustpreview");
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id)?;
    println!("CreateToastNotifierWithId: OK");
    println!("Setting before Show: {:?}", notifier.Setting());
    if !std::env::args().any(|a| a == "--send-test") {
        return Ok(());
    }
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from("<toast><visual><binding template=\"ToastGeneric\"><text>SpeedIpMsg diagnostic</text><text>Silent diagnostic; automatically removed.</text></binding></visual><audio silent=\"true\"/></toast>"))?;
    let toast = ToastNotification::CreateToastNotification(&document)?;
    let tag = HSTRING::from(format!(
        "{:x}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64
    ));
    let group = HSTRING::from("ipmsg-diagnostic");
    toast.SetTag(&tag)?;
    toast.SetGroup(&group)?;
    toast.SetSuppressPopup(true)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let token = toast.Failed(
        &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let _ = tx.send(args.ErrorCode());
            }
            Ok(())
        }),
    )?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        notifier.Show(&toast)?;
        println!("Show (SuppressPopup=true): OK");
        std::thread::sleep(Duration::from_millis(750));
        let failure = rx.try_recv();
        println!("Failed event: {failure:?}");
        let history = ToastNotificationManager::History()?.GetHistoryWithId(&app_id)?;
        let mut matches = 0;
        for i in 0..history.Size()? {
            let item = history.GetAt(i)?;
            if item.Tag()? == tag && item.Group()? == group {
                matches += 1;
            }
        }
        println!("Own notification in history: {matches}");
        println!("Setting after Show: {:?}", notifier.Setting());
        if matches != 1 || failure.is_ok() {
            return Err("notification was not accepted into history".into());
        }
        Ok(())
    })();
    let _ = toast.RemoveFailed(token);
    let _ = notifier.Hide(&toast);
    ToastNotificationManager::History()?.RemoveGroupedTagWithId(&tag, &group, &app_id)?;
    println!("Removed own diagnostic notification.");
    result
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows desktop only");
}
