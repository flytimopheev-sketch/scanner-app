//! Ручной дуплекс: сборка документа из двух проходов податчика.
//!
//! Сценарий «ручной перевёртки стопа»: сначала сканируются лицевые стороны
//! (1, 3, 5, …), затем стоп переворачивается и сканируются оборотные
//! стороны (2, 4, 6, … — они приходят в обратном порядке). Модуль
//! переплетает два списка в итоговый порядок документа.

/// Переплетение лицевых и оборотных страниц.
///
/// * `front` — страницы первого прохода в порядке сканирования;
/// * `back` — страницы второго прохода в порядке сканирования
///   (последняя отсканированная оборотная следует за первой лицевой).
///
/// Если оборотных страниц меньше (например, пустые отброшены), хвост
/// документа просто остаётся без оборотных. Лишние оборотные добавляются
/// в конец — пользователь легко их заметит и удалит.
pub fn interleave_duplex<T: Clone>(front: &[T], back: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(front.len() + back.len());
    for (i, f) in front.iter().enumerate() {
        out.push(f.clone());
        if let Some(b) = back.len().checked_sub(i + 1).and_then(|j| back.get(j)) {
            out.push(b.clone());
        }
    }
    // Оборотных оказалось больше, чем лицевых: дописываем остаток
    // (самые ранние из неспаренных) в конец.
    let paired = front.len().min(back.len());
    let extra_start = back.len() - paired;
    for b in back[..extra_start].iter() {
        out.push(b.clone());
    }
    out
}

/// Итоговое число страниц после переплетения.
pub fn duplex_page_count(front: usize, back: usize) -> usize {
    front + back
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_equal_halves() {
        // Лица: 1,3,5,7. Оборотные в порядке сканирования: 8,6,4,2.
        let front = vec![1, 3, 5, 7];
        let back = vec![8, 6, 4, 2];
        assert_eq!(interleave_duplex(&front, &back), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn interleave_missing_back_pages() {
        // Оборотных меньше: пустой оказалась оборотная ПОСЛЕДНЕГО листа,
        // при перевороте стопа она уходит в конец и не сканируется.
        // Лица: 1,3,5. Оборотные в порядке сканирования: 4,2 (6 пропущена).
        let front = vec![1, 3, 5];
        let back = vec![4, 2];
        assert_eq!(interleave_duplex(&front, &back), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn interleave_extra_back_pages() {
        let front = vec![1, 3];
        let back = vec![6, 4, 2];
        assert_eq!(interleave_duplex(&front, &back), vec![1, 2, 3, 4, 6]);
    }

    #[test]
    fn interleave_single_sided_fallback() {
        // Пользователь отсканировал только лица (отмена второго прохода).
        let front = vec!["a", "b"];
        let back: Vec<&str> = Vec::new();
        assert_eq!(interleave_duplex(&front, &back), vec!["a", "b"]);
    }
}
