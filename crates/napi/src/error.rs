use std::convert::{From, TryFrom};
use std::error;
use std::ffi::{CStr, CString};
use std::fmt;
#[cfg(feature = "serde-json")]
use std::fmt::Display;
use std::os::raw::{c_char, c_void};
use std::ptr;

#[cfg(feature = "serde-json")]
use serde::{de, ser};
#[cfg(feature = "serde-json")]
use serde_json::Error as SerdeJSONError;

use crate::bindgen_runtime::ToNapiValue;
use crate::{check_status, sys, Env, JsUnknown, NapiValue, Status};

pub type Result<T> = std::result::Result<T, Error>;

/// Represent `JsError`.
/// Return this Error in `js_function`, **napi-rs** will throw it as `JsError` for you.
/// If you want throw it as `TypeError` or `RangeError`, you can use `JsTypeError/JsRangeError::from(Error).throw_into(env)`
#[derive(Debug, Clone)]
pub struct Error {
  pub status: Status,
  pub reason: String,
  // Convert raw `JsError` into Error
  maybe_raw: sys::napi_ref,
  maybe_env: sys::napi_env,
}

impl ToNapiValue for Error {
  unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
    if val.maybe_raw.is_null() {
      let err = unsafe { JsError::from(val).into_value(env) };
      Ok(err)
    } else {
      let mut value = std::ptr::null_mut();
      check_status!(unsafe {
        sys::napi_get_reference_value(val.maybe_env, val.maybe_raw, &mut value)
      })?;
      Ok(value)
    }
  }
}

unsafe impl Send for Error {}
unsafe impl Sync for Error {}

impl error::Error for Error {}

impl From<std::convert::Infallible> for Error {
  fn from(_: std::convert::Infallible) -> Self {
    unreachable!()
  }
}

#[cfg(feature = "serde-json")]
impl ser::Error for Error {
  fn custom<T: Display>(msg: T) -> Self {
    Error::new(Status::InvalidArg, msg.to_string())
  }
}

#[cfg(feature = "serde-json")]
impl de::Error for Error {
  fn custom<T: Display>(msg: T) -> Self {
    Error::new(Status::InvalidArg, msg.to_string())
  }
}

#[cfg(feature = "serde-json")]
impl From<SerdeJSONError> for Error {
  fn from(value: SerdeJSONError) -> Self {
    Error::new(Status::InvalidArg, format!("{}", value))
  }
}

impl From<JsUnknown> for Error {
  fn from(value: JsUnknown) -> Self {
    let mut result = std::ptr::null_mut();
    let status = unsafe { sys::napi_create_reference(value.0.env, value.0.value, 0, &mut result) };
    if status != sys::Status::napi_ok {
      return Error::new(Status::from(status), "".to_owned());
    }
    Self {
      status: Status::GenericFailure,
      reason: "".to_string(),
      maybe_raw: result,
      maybe_env: value.0.env,
    }
  }
}

#[cfg(feature = "anyhow")]
impl From<anyhow::Error> for Error {
  fn from(value: anyhow::Error) -> Self {
    Error::new(Status::GenericFailure, format!("{}", value))
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if !self.reason.is_empty() {
      write!(f, "{:?}, {}", self.status, self.reason)
    } else {
      write!(f, "{:?}", self.status)
    }
  }
}

impl Error {
  pub fn new(status: Status, reason: String) -> Self {
    Error {
      status,
      reason,
      maybe_raw: ptr::null_mut(),
      maybe_env: ptr::null_mut(),
    }
  }

  pub fn from_status(status: Status) -> Self {
    Error {
      status,
      reason: "".to_owned(),
      maybe_raw: ptr::null_mut(),
      maybe_env: ptr::null_mut(),
    }
  }

  pub fn from_reason<T: Into<String>>(reason: T) -> Self {
    Error {
      status: Status::GenericFailure,
      reason: reason.into(),
      maybe_raw: ptr::null_mut(),
      maybe_env: ptr::null_mut(),
    }
  }
}

impl From<std::ffi::NulError> for Error {
  fn from(error: std::ffi::NulError) -> Self {
    Error {
      status: Status::GenericFailure,
      reason: format!("{}", error),
      maybe_raw: ptr::null_mut(),
      maybe_env: ptr::null_mut(),
    }
  }
}

impl From<std::io::Error> for Error {
  fn from(error: std::io::Error) -> Self {
    Error {
      status: Status::GenericFailure,
      reason: format!("{}", error),
      maybe_raw: ptr::null_mut(),
      maybe_env: ptr::null_mut(),
    }
  }
}

impl Drop for Error {
  fn drop(&mut self) {
    #[cfg(not(feature = "noop"))]
    {
      if !self.maybe_env.is_null() && !self.maybe_raw.is_null() {
        let delete_reference_status =
          unsafe { sys::napi_delete_reference(self.maybe_env, self.maybe_raw) };
        debug_assert!(
          delete_reference_status == sys::Status::napi_ok,
          "Delete Error Reference failed"
        );
      }
    }
  }
}

#[derive(Clone, Debug)]
pub struct ExtendedErrorInfo {
  pub message: String,
  pub engine_reserved: *mut c_void,
  pub engine_error_code: u32,
  pub error_code: Status,
}

impl TryFrom<sys::napi_extended_error_info> for ExtendedErrorInfo {
  type Error = Error;

  fn try_from(value: sys::napi_extended_error_info) -> Result<Self> {
    Ok(Self {
      message: unsafe {
        CString::from_raw(value.error_message as *mut c_char)
          .into_string()
          .map_err(|e| Error::new(Status::GenericFailure, format!("{}", e)))?
      },
      engine_error_code: value.engine_error_code,
      engine_reserved: value.engine_reserved,
      error_code: Status::from(value.error_code),
    })
  }
}

pub struct JsError(Error);

#[cfg(feature = "anyhow")]
impl From<anyhow::Error> for JsError {
  fn from(value: anyhow::Error) -> Self {
    JsError(Error::new(Status::GenericFailure, value.to_string()))
  }
}

pub struct JsTypeError(Error);

pub struct JsRangeError(Error);

#[cfg(feature = "experimental")]
pub struct JsSyntaxError(Error);

macro_rules! impl_object_methods {
  ($js_value:ident, $kind:expr) => {
    impl $js_value {
      /// # Safety
      ///
      /// This function is safety if env is not null ptr.
      pub unsafe fn into_value(self, env: sys::napi_env) -> sys::napi_value {
        if !self.0.maybe_raw.is_null() {
          let mut err = ptr::null_mut();
          let get_err_status =
            unsafe { sys::napi_get_reference_value(env, self.0.maybe_raw, &mut err) };
          debug_assert!(
            get_err_status == sys::Status::napi_ok,
            "Get Error from Reference failed"
          );
          return err;
        }

        let error_status = format!("{:?}", self.0.status);
        let status_len = error_status.len();
        let error_code_string = CString::new(error_status).unwrap();
        let reason_len = self.0.reason.len();
        let reason = CString::new(self.0.reason.as_str()).unwrap();
        let mut error_code = ptr::null_mut();
        let mut reason_string = ptr::null_mut();
        let mut js_error = ptr::null_mut();
        let create_code_status = unsafe {
          sys::napi_create_string_utf8(env, error_code_string.as_ptr(), status_len, &mut error_code)
        };
        debug_assert!(create_code_status == sys::Status::napi_ok);
        let create_reason_status = unsafe {
          sys::napi_create_string_utf8(env, reason.as_ptr(), reason_len, &mut reason_string)
        };
        debug_assert!(create_reason_status == sys::Status::napi_ok);
        let create_error_status = unsafe { $kind(env, error_code, reason_string, &mut js_error) };
        debug_assert!(create_error_status == sys::Status::napi_ok);
        js_error
      }

      pub fn into_unknown(self, env: Env) -> JsUnknown {
        let value = unsafe { self.into_value(env.raw()) };
        unsafe { JsUnknown::from_raw_unchecked(env.raw(), value) }
      }

      /// # Safety
      ///
      /// This function is safety if env is not null ptr.
      pub unsafe fn throw_into(self, env: sys::napi_env) {
        #[cfg(debug_assertions)]
        let reason = self.0.reason.clone();
        let status = self.0.status;
        if status == Status::PendingException {
          return;
        }
        let js_error = unsafe { self.into_value(env) };
        #[cfg(debug_assertions)]
        let throw_status = unsafe { sys::napi_throw(env, js_error) };
        unsafe { sys::napi_throw(env, js_error) };
        #[cfg(debug_assertions)]
        assert!(
          throw_status == sys::Status::napi_ok,
          "Throw error failed, status: [{}], raw message: \"{}\", raw status: [{}]",
          Status::from(throw_status),
          reason,
          Status::from(status)
        );
      }

      #[allow(clippy::not_unsafe_ptr_arg_deref)]
      pub fn throw(&self, env: sys::napi_env) -> Result<()> {
        let error_status = format!("{:?}\0", self.0.status);
        let status_len = error_status.len();
        let error_code_string =
          unsafe { CStr::from_bytes_with_nul_unchecked(error_status.as_bytes()) };
        let reason_len = self.0.reason.len();
        let reason_c_string = format!("{}\0", self.0.reason.clone());
        let reason = unsafe { CStr::from_bytes_with_nul_unchecked(reason_c_string.as_bytes()) };
        let mut error_code = ptr::null_mut();
        let mut reason_string = ptr::null_mut();
        let mut js_error = ptr::null_mut();
        check_status!(unsafe {
          sys::napi_create_string_utf8(env, error_code_string.as_ptr(), status_len, &mut error_code)
        })?;
        check_status!(unsafe {
          sys::napi_create_string_utf8(env, reason.as_ptr(), reason_len, &mut reason_string)
        })?;
        check_status!(unsafe { $kind(env, error_code, reason_string, &mut js_error) })?;
        check_status!(unsafe { sys::napi_throw(env, js_error) })
      }
    }

    impl From<Error> for $js_value {
      fn from(err: Error) -> Self {
        Self(err)
      }
    }

    impl crate::bindgen_prelude::ToNapiValue for $js_value {
      unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        unsafe { ToNapiValue::to_napi_value(env, val.0) }
      }
    }
  };
}

impl_object_methods!(JsError, sys::napi_create_error);
impl_object_methods!(JsTypeError, sys::napi_create_type_error);
impl_object_methods!(JsRangeError, sys::napi_create_range_error);
#[cfg(feature = "experimental")]
impl_object_methods!(JsSyntaxError, sys::node_api_create_syntax_error);

#[doc(hidden)]
#[macro_export]
macro_rules! error {
  ($status:expr, $($msg:tt)*) => {
    $crate::Error::new($status, format!($($msg)*))
  };
}

#[doc(hidden)]
#[macro_export]
macro_rules! check_status {
  ($code:expr) => {{
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      _ => Err($crate::Error::new($crate::Status::from(c), "".to_owned())),
    }
  }};

  ($code:expr, $($msg:tt)*) => {{
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      _ => Err($crate::Error::new($crate::Status::from(c), format!($($msg)*))),
    }
  }};

  ($code:expr, $msg:expr, $env:expr, $val:expr) => {{
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      _ => Err($crate::Error::new($crate::Status::from(c), format!($msg, $crate::type_of!($env, $val)?))),
    }
  }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! check_status_and_type {
  ($code:expr, $env:ident, $val:ident, $msg:expr) => {{
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      _ => {
        use $crate::js_values::NapiValue;
        let value_type = $crate::type_of!($env, $val)?;
        let error_msg = match value_type {
          ValueType::Function => {
            let function_name = unsafe { JsFunction::from_raw_unchecked($env, $val).name()? };
            format!(
              $msg,
              format!(
                "function {}(..) ",
                if function_name.len() == 0 {
                  "anonymous".to_owned()
                } else {
                  function_name
                }
              )
            )
          }
          ValueType::Object => {
            let env_ = $crate::Env::from($env);
            let json: $crate::JSON = env_.get_global()?.get_named_property_unchecked("JSON")?;
            let object = json.stringify($crate::JsObject($crate::Value {
              value: $val,
              env: $env,
              value_type: ValueType::Object,
            }))?;
            format!($msg, format!("Object {}", object))
          }
          ValueType::Boolean | ValueType::Number => {
            let value =
              unsafe { $crate::JsUnknown::from_raw_unchecked($env, $val).coerce_to_string()? }
                .into_utf8()?;
            format!($msg, format!("{} {} ", value_type, value.as_str()?))
          }
          #[cfg(feature = "napi6")]
          ValueType::BigInt => {
            let value =
              unsafe { $crate::JsUnknown::from_raw_unchecked($env, $val).coerce_to_string()? }
                .into_utf8()?;
            format!($msg, format!("{} {} ", value_type, value.as_str()?))
          }
          _ => format!($msg, value_type),
        };
        Err($crate::Error::new($crate::Status::from(c), error_msg))
      }
    }
  }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! check_pending_exception {
  ($env:expr, $code:expr) => {{
    use $crate::NapiValue;
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      $crate::sys::Status::napi_pending_exception => {
        let mut error_result = std::ptr::null_mut();
        assert_eq!(
          unsafe { $crate::sys::napi_get_and_clear_last_exception($env, &mut error_result) },
          $crate::sys::Status::napi_ok
        );
        return Err($crate::Error::from(unsafe {
          $crate::bindgen_prelude::Unknown::from_raw_unchecked($env, error_result)
        }));
      }
      _ => Err($crate::Error::new($crate::Status::from(c), "".to_owned())),
    }
  }};

  ($env:expr, $code:expr, $($msg:tt)*) => {{
    use $crate::NapiValue;
    let c = $code;
    match c {
      $crate::sys::Status::napi_ok => Ok(()),
      $crate::sys::Status::napi_pending_exception => {
        let mut error_result = std::ptr::null_mut();
        assert_eq!(
          unsafe { $crate::sys::napi_get_and_clear_last_exception($env, &mut error_result) },
          $crate::sys::Status::napi_ok
        );
        return Err($crate::Error::from(unsafe {
          $crate::bindgen_prelude::Unknown::from_raw_unchecked($env, error_result)
        }));
      }
      _ => Err($crate::Error::new($crate::Status::from(c), format!($($msg)*))),
    }
  }};
}
