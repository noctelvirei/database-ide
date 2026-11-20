//! Constant-memory Excel output, preserving values Excel cannot represent as text.

use std::fs::File;

use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, Worksheet};

use crate::session::{Column, Value};

/// Workbook creation, cell encoding or finalization failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Excel's cell or worksheet bounds were exceeded, or workbook I/O failed.
    #[error("Workbook export failed: {0}")]
    Xlsx(#[from] rust_xlsxwriter::XlsxError),
    /// File synchronization failed.
    #[error("Workbook file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// Excel permits at most 16,384 columns.
    #[error("Excel supports at most 16384 columns")]
    Columns,
}

pub(super) struct Writer {
    workbook: Workbook,
    file: File,
    columns: Vec<Column>,
    sheet: usize,
    row: u32,
    date: Format,
    timestamp: Format,
}

fn header(worksheet: &mut Worksheet, columns: &[Column]) -> Result<(), Error> {
    for (column, metadata) in columns.iter().enumerate() {
        worksheet.write_string(0, column as u16, &metadata.name)?;
    }
    Ok(())
}

fn supported_number(number: f64) -> bool {
    number == 0.0 || (f64::MIN_POSITIVE..=9.999_999_999_999_99e307).contains(&number.abs())
}

fn exact_number(text: &str) -> Option<f64> {
    let digits: String = text
        .split(['e', 'E'])
        .next()?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let significant = digits.trim_matches('0').len();
    let number = text.parse::<f64>().ok()?;
    (supported_number(number) && significant <= 15 && (number != 0.0 || significant == 0))
        .then_some(number)
}

impl Writer {
    pub(super) fn new(file: File, columns: &[Column]) -> Result<Self, Error> {
        if columns.len() > 16_384 {
            return Err(Error::Columns);
        }
        let mut workbook = Workbook::new();
        header(workbook.add_worksheet_with_constant_memory()?, columns)?;
        Ok(Self {
            workbook,
            file,
            columns: columns.to_owned(),
            sheet: 0,
            row: 1,
            date: Format::new().set_num_format("yyyy-mm-dd"),
            timestamp: Format::new().set_num_format("yyyy-mm-dd hh:mm:ss.000"),
        })
    }

    pub(super) fn row(&mut self, values: &[Value]) -> Result<(), Error> {
        if self.row == 1_048_576 {
            header(
                self.workbook.add_worksheet_with_constant_memory()?,
                &self.columns,
            )?;
            self.sheet += 1;
            self.row = 1;
        }
        let worksheet = self.workbook.worksheet_from_index(self.sheet)?;
        for (column, value) in values.iter().enumerate() {
            let column = column as u16;
            match value {
                Value::Null => {}
                Value::Bool(value) => {
                    worksheet.write_boolean(self.row, column, *value)?;
                }
                Value::Float(value) if supported_number(*value) => {
                    worksheet.write_number(self.row, column, *value)?;
                }
                Value::Integer(_) | Value::UnsignedInteger(_) | Value::Decimal(_) => {
                    let text = super::cell_text(value);
                    if let Some(number) = exact_number(&text) {
                        worksheet.write_number(self.row, column, number)?;
                    } else {
                        worksheet.write_string(self.row, column, &text)?;
                    }
                }
                Value::Date(_) | Value::Timestamp(_) => {
                    let text = super::cell_text(value);
                    match ExcelDateTime::parse_from_str(&text) {
                        Ok(date) => {
                            let format = if matches!(value, Value::Date(_)) {
                                &self.date
                            } else {
                                &self.timestamp
                            };
                            worksheet
                                .write_datetime_with_format(self.row, column, &date, format)?;
                        }
                        Err(_) => {
                            worksheet.write_string(self.row, column, &text)?;
                        }
                    }
                }
                _ => {
                    worksheet.write_string(self.row, column, super::cell_text(value))?;
                }
            }
        }
        self.row += 1;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(), Error> {
        self.workbook.save_to_writer(&mut self.file)?;
        self.file.sync_all()?;
        Ok(())
    }
}
