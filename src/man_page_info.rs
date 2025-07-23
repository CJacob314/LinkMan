use std::error::Error;
use std::ffi::CString;
use std::fmt::{self, Display, Formatter};

pub(crate) struct ManPageInfo<'a> {
    name: &'a str,
    section_number: &'a str,
}

#[derive(Debug)]
pub(crate) struct StringNotManRefError;

impl Display for StringNotManRefError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "StringNotManRefError")
    }
}

impl Error for StringNotManRefError {}

impl<'a> TryFrom<&'a str> for ManPageInfo<'a> {
    type Error = StringNotManRefError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        // Disallow path separator and U+0000
        if value.chars().any(|c| c == '\x00' || c == '/') {
            return Err(StringNotManRefError);
        }

        if let Some(open_paren_index) = value.find('(') {
            let next_char = &value[open_paren_index..]
                .chars()
                .nth(1)
                .ok_or(StringNotManRefError)?;
            next_char
                .is_ascii_digit()
                .then(|| {
                    Ok(ManPageInfo {
                        name: &value[..open_paren_index],
                        section_number: &value[(open_paren_index + 1)..(open_paren_index + 2)], // This subslicing is safe since we know
                                                                                                // next_char was an ASCII digit
                    })
                })
                .ok_or(StringNotManRefError)?
        } else {
            Err(StringNotManRefError)
        }
    }
}

impl<'a> Display for ManPageInfo<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.section_number, self.name)
    }
}

impl<'a> ManPageInfo<'a> {
    pub(crate) fn as_args(&self) -> anyhow::Result<(CString, CString)> {
        Ok((CString::new(self.section_number)?, CString::new(self.name)?))
    }
}
