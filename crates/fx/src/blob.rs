//! Двоичный буфер с **явными размерами полей**: запись и чтение.
//!
//! Существует ради сохранения партии (ADR-229). `serde` поверх живых
//! структур сюда не годится: раскладка тогда задаётся тем, как сегодня
//! объявлены поля, и правка структуры молча меняет формат файла.
//! Здесь каждое поле пишется руками, своей шириной, и порядок задан
//! исходником сохранения, а не выводом макроса.
//!
//! Живёт в `fx`, потому что от него зависят все крейты симуляции,
//! а слой сохранения им всем и нужен. Плавающего здесь нет, файлов
//! здесь нет — это буфер байт.
//!
//! **Порядок байт — от младшего.** Он же у обоих наших целевых
//! платформ, и запись через `to_le_bytes` делает файл переносимым
//! даже туда, где это не так.
//!
//! **Список всегда начинается длиной `u32`.** Читатель, дошедший
//! до конца буфера, не паникует и не режет: он ставит [`Scan::bad`]
//! и возвращает нули. Проверка одна, в конце разбора.

use crate::Fx;

/// Запись.
#[derive(Default)]
pub struct Blob {
    pub v: Vec<u8>,
}

impl Blob {
    pub fn new() -> Blob {
        Blob { v: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    pub fn u8(&mut self, x: u8) {
        self.v.push(x);
    }

    pub fn bool(&mut self, x: bool) {
        self.v.push(u8::from(x));
    }

    pub fn u16(&mut self, x: u16) {
        self.v.extend_from_slice(&x.to_le_bytes());
    }

    pub fn u32(&mut self, x: u32) {
        self.v.extend_from_slice(&x.to_le_bytes());
    }

    pub fn u64(&mut self, x: u64) {
        self.v.extend_from_slice(&x.to_le_bytes());
    }

    /// Знаковое пишется битами, а не текстом: `as u32` — это перевод
    /// в дополнительный код, и он обратим.
    pub fn i32(&mut self, x: i32) {
        self.u32(x as u32);
    }

    pub fn i64(&mut self, x: i64) {
        self.u64(x as u64);
    }

    pub fn usize(&mut self, x: usize) {
        self.u64(x as u64);
    }

    /// `Fx` — сырым `i64`. Единственно возможный способ: это и есть
    /// число, а не его десятичная запись.
    pub fn fx(&mut self, x: Fx) {
        self.i64(x.raw());
    }

    /// Число переменной длины, LEB128: по семь бит в байте, старший
    /// бит — «продолжение».
    ///
    /// Не нарушает правила «явные размеры полей»: длина задаётся
    /// самим числом по общеизвестной раскладке, а не тем, как сегодня
    /// объявлено поле. Нужна очереди правок мира — она бывает
    /// в миллионы записей, и три четверти их байт там нули (ADR-229).
    pub fn uvar(&mut self, mut x: u64) {
        loop {
            let b = (x & 0x7f) as u8;
            x >>= 7;
            if x == 0 {
                self.v.push(b);
                return;
            }
            self.v.push(b | 0x80);
        }
    }

    /// Знаковое переменной длины: зигзаг, потом LEB128. Зигзаг нужен,
    /// потому что разности идут в обе стороны, а дополнительный код
    /// делает −1 самым длинным числом из возможных.
    pub fn ivar(&mut self, x: i64) {
        self.uvar(((x << 1) ^ (x >> 63)) as u64);
    }

    pub fn u8s(&mut self, s: &[u8]) {
        self.u32(s.len() as u32);
        self.v.extend_from_slice(s);
    }

    /// Логические — по биту, а не по байту: их бывают миллионы.
    pub fn bools(&mut self, s: &[bool]) {
        self.u32(s.len() as u32);
        let mut byte = 0u8;
        for (i, &b) in s.iter().enumerate() {
            if b {
                byte |= 1 << (i & 7);
            }
            if i & 7 == 7 {
                self.v.push(byte);
                byte = 0;
            }
        }
        if s.len() & 7 != 0 {
            self.v.push(byte);
        }
    }

    pub fn u16s(&mut self, s: &[u16]) {
        self.u32(s.len() as u32);
        for &x in s {
            self.u16(x);
        }
    }

    pub fn u32s(&mut self, s: &[u32]) {
        self.u32(s.len() as u32);
        for &x in s {
            self.u32(x);
        }
    }

    pub fn u64s(&mut self, s: &[u64]) {
        self.u32(s.len() as u32);
        for &x in s {
            self.u64(x);
        }
    }

    pub fn i32s(&mut self, s: &[i32]) {
        self.u32(s.len() as u32);
        for &x in s {
            self.i32(x);
        }
    }

    pub fn fxs(&mut self, s: &[Fx]) {
        self.u32(s.len() as u32);
        for &x in s {
            self.fx(x);
        }
    }
}

/// Чтение. Недочитанное — не паника, а [`Scan::bad`].
pub struct Scan<'a> {
    b: &'a [u8],
    at: usize,
    bad: bool,
}

impl<'a> Scan<'a> {
    pub fn new(b: &'a [u8]) -> Scan<'a> {
        Scan {
            b,
            at: 0,
            bad: false,
        }
    }

    /// Целы ли данные. Единственная проверка, которая нужна: всё,
    /// что читалось за концом буфера, вернуло ноль и подняло этот флаг.
    pub fn ok(&self) -> bool {
        !self.bad
    }

    /// Сколько байт осталось непрочитанными.
    pub fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }

    /// Объявить данные испорченными.
    ///
    /// Нужна разборщикам, которые видят бессмыслицу **по смыслу**,
    /// а не по длине: отрезок с отрицательным началом, список, длина
    /// которого не сошлась с содержимым. Без неё такой разборщик
    /// возвращал бы пустоту молча, и обрезанный файл читался бы как
    /// пустая партия.
    pub fn spoil(&mut self) {
        self.bad = true;
    }

    fn take(&mut self, n: usize) -> &[u8] {
        if self.at + n > self.b.len() {
            self.bad = true;
            self.at = self.b.len();
            return &[];
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        s
    }

    pub fn u8(&mut self) -> u8 {
        match self.take(1) {
            [x] => *x,
            _ => 0,
        }
    }

    pub fn bool(&mut self) -> bool {
        self.u8() != 0
    }

    pub fn u16(&mut self) -> u16 {
        match self.take(2).try_into() {
            Ok(a) => u16::from_le_bytes(a),
            Err(_) => 0,
        }
    }

    pub fn u32(&mut self) -> u32 {
        match self.take(4).try_into() {
            Ok(a) => u32::from_le_bytes(a),
            Err(_) => 0,
        }
    }

    pub fn u64(&mut self) -> u64 {
        match self.take(8).try_into() {
            Ok(a) => u64::from_le_bytes(a),
            Err(_) => 0,
        }
    }

    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    pub fn i64(&mut self) -> i64 {
        self.u64() as i64
    }

    pub fn usize(&mut self) -> usize {
        self.u64() as usize
    }

    pub fn fx(&mut self) -> Fx {
        Fx(self.i64())
    }

    /// Число переменной длины. Больше десяти байт подряд — порча:
    /// в `u64` семибитных групп ровно десять.
    pub fn uvar(&mut self) -> u64 {
        let mut x = 0u64;
        for k in 0..10u32 {
            let b = self.u8();
            x |= u64::from(b & 0x7f) << (k * 7);
            if b & 0x80 == 0 {
                return x;
            }
        }
        self.bad = true;
        0
    }

    pub fn ivar(&mut self) -> i64 {
        let u = self.uvar();
        ((u >> 1) as i64) ^ -((u & 1) as i64)
    }

    /// Длина списка. Заведомо невозможная длина — это порча файла,
    /// а не запрос на выделение гигабайта: она отсекается по остатку
    /// буфера, потому что короче байта элемента не бывает.
    fn count(&mut self, min_bytes: usize) -> usize {
        let n = self.u32() as usize;
        if n.saturating_mul(min_bytes.max(1)) > self.left().saturating_add(min_bytes) {
            self.bad = true;
            return 0;
        }
        n
    }

    pub fn u8s(&mut self) -> Vec<u8> {
        let n = self.count(1);
        self.take(n).to_vec()
    }

    pub fn bools(&mut self) -> Vec<bool> {
        let n = self.u32() as usize;
        let bytes = n.div_ceil(8);
        if bytes > self.left() {
            self.bad = true;
            return Vec::new();
        }
        let src = self.take(bytes).to_vec();
        (0..n).map(|i| src[i >> 3] & (1 << (i & 7)) != 0).collect()
    }

    pub fn u16s(&mut self) -> Vec<u16> {
        let n = self.count(2);
        (0..n).map(|_| self.u16()).collect()
    }

    pub fn u32s(&mut self) -> Vec<u32> {
        let n = self.count(4);
        (0..n).map(|_| self.u32()).collect()
    }

    pub fn u64s(&mut self) -> Vec<u64> {
        let n = self.count(8);
        (0..n).map(|_| self.u64()).collect()
    }

    pub fn i32s(&mut self) -> Vec<i32> {
        let n = self.count(4);
        (0..n).map(|_| self.i32()).collect()
    }

    pub fn fxs(&mut self) -> Vec<Fx> {
        let n = self.count(8);
        (0..n).map(|_| self.fx()).collect()
    }

    /// Длина списка составных записей. Читателю остаётся цикл.
    pub fn list(&mut self, min_bytes: usize) -> usize {
        self.count(min_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Записанное читается тем же и в том же порядке.
    #[test]
    fn what_was_written_reads_back_the_same() {
        let mut b = Blob::new();
        b.u8(7);
        b.bool(true);
        b.u16(0xBEEF);
        b.u32(0xDEAD_BEEF);
        b.u64(0x0123_4567_89AB_CDEF);
        b.i32(-5);
        b.i64(i64::MIN);
        b.fx(Fx::from_ratio(-1, 3));
        b.u8s(&[1, 2, 3]);
        b.bools(&[true, false, true, true, false, false, false, false, true]);
        b.u16s(&[9, 8, 7]);
        b.u32s(&[1, 2]);
        b.u64s(&[u64::MAX]);
        b.i32s(&[-1, 0, 1]);
        b.fxs(&[Fx::ONE, Fx::ZERO]);

        let mut s = Scan::new(&b.v);
        assert_eq!(s.u8(), 7);
        assert!(s.bool());
        assert_eq!(s.u16(), 0xBEEF);
        assert_eq!(s.u32(), 0xDEAD_BEEF);
        assert_eq!(s.u64(), 0x0123_4567_89AB_CDEF);
        assert_eq!(s.i32(), -5);
        assert_eq!(s.i64(), i64::MIN);
        assert_eq!(s.fx(), Fx::from_ratio(-1, 3));
        assert_eq!(s.u8s(), vec![1, 2, 3]);
        assert_eq!(
            s.bools(),
            vec![true, false, true, true, false, false, false, false, true]
        );
        assert_eq!(s.u16s(), vec![9, 8, 7]);
        assert_eq!(s.u32s(), vec![1, 2]);
        assert_eq!(s.u64s(), vec![u64::MAX]);
        assert_eq!(s.i32s(), vec![-1, 0, 1]);
        assert_eq!(s.fxs(), vec![Fx::ONE, Fx::ZERO]);
        assert!(s.ok(), "разбор не должен был испортиться");
        assert_eq!(s.left(), 0, "прочитано не всё");
    }

    /// **Обрезанный файл не паникует, а признаётся испорченным.**
    /// Это важнее удобства: сохранение читают с диска, а диск врёт.
    #[test]
    fn a_truncated_buffer_is_bad_not_a_panic() {
        let mut b = Blob::new();
        b.u32s(&[1, 2, 3, 4, 5]);
        for cut in 0..b.v.len() {
            let mut s = Scan::new(&b.v[..cut]);
            let _ = s.u32s();
            assert!(!s.ok(), "обрезка на {cut} байте прошла незамеченной");
        }
    }

    /// Невозможная длина списка отсекается **до** выделения памяти.
    #[test]
    fn an_impossible_length_is_rejected_before_allocating() {
        let mut b = Blob::new();
        b.u32(u32::MAX);
        let mut s = Scan::new(&b.v);
        assert!(s.u64s().is_empty());
        assert!(!s.ok());
    }

    /// Логические пакуются по биту: тысяча значений — сто двадцать
    /// пять байт плюс длина, а не тысяча.
    #[test]
    fn bools_are_packed_by_the_bit() {
        let mut b = Blob::new();
        b.bools(&vec![true; 1000]);
        assert_eq!(b.len(), 4 + 125);
    }

    /// Числа переменной длины читаются обратно точно, включая края.
    #[test]
    fn varints_round_trip_including_the_edges() {
        let us = [0u64, 1, 127, 128, 16383, 16384, u32::MAX as u64, u64::MAX];
        let is = [0i64, -1, 1, -64, 63, i32::MIN as i64, i64::MIN, i64::MAX];
        let mut b = Blob::new();
        for &x in &us {
            b.uvar(x);
        }
        for &x in &is {
            b.ivar(x);
        }
        let mut s = Scan::new(&b.v);
        for &x in &us {
            assert_eq!(s.uvar(), x);
        }
        for &x in &is {
            assert_eq!(s.ivar(), x);
        }
        assert!(s.ok());
        assert_eq!(s.left(), 0);
    }

    /// Маленькое число — один байт. Ради этого всё и затевалось.
    #[test]
    fn a_small_varint_takes_one_byte() {
        let mut b = Blob::new();
        b.uvar(100);
        b.ivar(-5);
        assert_eq!(b.len(), 2);
    }

    /// Бесконечное «продолжение» — порча, а не вечный цикл.
    #[test]
    fn an_endless_varint_is_bad() {
        let mut s = Scan::new(&[0x80u8; 32]);
        let _ = s.uvar();
        assert!(!s.ok());
    }
}
