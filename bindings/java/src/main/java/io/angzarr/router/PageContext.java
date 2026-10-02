package io.angzarr.router;

import io.angzarr.Cover;

/**
 * Where a folded event sits: its book's cover and the page's explicit sequence.
 *
 * @param cover the delivered book's cover (the default instance when the book carries none)
 * @param sequence the page's explicit sequence, 0 when the page carries none
 */
public record PageContext(Cover cover, long sequence) {}
