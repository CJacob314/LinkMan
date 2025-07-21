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

impl<'a> TryInto<ManPageInfo<'a>> for &'a str {
    type Error = StringNotManRefError;

    fn try_into(self) -> Result<ManPageInfo<'a>, Self::Error> {
        // Disallow path separator and U+0000
        if self.chars().any(|c| c == '\x00' || c == '/') {
            return Err(StringNotManRefError);
        }

        if let Some(open_paren_index) = self.find('(') {
            let next_char = &self[open_paren_index..]
                .chars()
                .nth(1)
                .ok_or(StringNotManRefError)?;
            next_char
                .is_ascii_digit()
                .then(|| {
                    Ok(ManPageInfo {
                        name: &self[..open_paren_index],
                        section_number: &self[(open_paren_index + 1)..(open_paren_index + 2)], // This subslicing is safe since we know
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
