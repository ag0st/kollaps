use std::borrow::BorrowMut;
use std::ops::{Index, IndexMut};

use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Matrix<T: Copy> {
    n: usize,
    data: Vec<Vec<T>>,
}

impl<T: Copy> IndexMut<(usize, usize)> for Matrix<T> {
    fn index_mut(&mut self, index: (usize, usize)) -> &mut Self::Output {
        let mut ind = index;
        if ind.0 > ind.1 {
            std::mem::swap(&mut ind.0, &mut ind.1)
        }
        self.data[ind.0][ind.1].borrow_mut()
    }
}

impl<T: Copy> Index<(usize, usize)> for Matrix<T> {
    type Output = T;

    fn index(&self, index: (usize, usize)) -> &Self::Output {
        let mut ind = index;
        if ind.0 > ind.1 {
            std::mem::swap(&mut ind.0, &mut ind.1)
        }
        &self.data[ind.0][ind.1]
    }
}


impl<T: Copy> Matrix<T> {
    pub fn new_fn<F>(n: usize, f: F) -> Self
        where F: Fn(usize, usize) -> T {
        let data = (0..n)
            .map(|row| (0..n)
                .map(|col| { f(row, col) })
                .collect())
            .collect();

        Matrix { n, data }
    }
    pub fn size(&self) -> usize {
        self.n
    }

    pub fn grow_fn<F>(&self, by: usize, f: F) -> Self
        where F: Fn(usize, usize) -> T {
        // create a new matrix of the new size and directly copy the element of this one
        Matrix::new_fn(self.n + by, |row, col| {
            if row >= self.n || col >= self.n {
                // this is added row, execute our f
                f(row, col)
            } else {
                // copy our content to the new matrix
                self[(row, col)]
            }
        })
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SymMatrix<T: Copy> {
    n: usize,
    data: Vec<T>,
}

impl<T: Copy> IndexMut<(usize, usize)> for SymMatrix<T> {
    fn index_mut(&mut self, index: (usize, usize)) -> &mut Self::Output {
        let index_in_vec = SymMatrix::<T>::out_to_in_index(index.0, index.1, self.n);
        self.data[index_in_vec].borrow_mut()
    }
}

impl<T: Copy> Index<(usize, usize)> for SymMatrix<T> {
    type Output = T;

    fn index(&self, index: (usize, usize)) -> &Self::Output {
        let index_in_vec = SymMatrix::<T>::out_to_in_index(index.0, index.1, self.n);
        &self.data[index_in_vec]
    }
}

impl<T: Copy> SymMatrix<T> {
    pub fn new_fn<F>(n: usize, mut f: F) -> Self
        where F: FnMut(usize, usize) -> T {
        // The number of element in the vector is given by the arithmetic sequence of reason = n
        // size = n * ((1 + 1 + (n - 1) * 1) / 2) == n * ((n + 1) / 2)
        let x = n as f64 * ((n as f64 + 1.0) / 2.0);
        let size_vec = x as usize;
        let mut data = Vec::new();
        for i in 0..size_vec {
            let (row, col) = SymMatrix::<T>::in_to_out_index(i, n);
            data.push(f(row, col))
        }
        SymMatrix {
            n,
            data,
        }
    }
    pub fn size(&self) -> usize {
        self.n
    }
    fn out_to_in_index(row: usize, col: usize, n: usize) -> usize {
        // index in the upper triangle of the matrix
        // as the matrix is symmetrical, row * n + col = col * n + row,
        // by choosing the row as the smaller coordinate, we will always be in the
        // upper triangle of the matrix if applying row * n + col.
        let mut row = row;
        let mut col = col;
        if row > col {
            (row, col) = (col, row);
        }

        // arithmetic sequence for calculating the number of elements composing the past rows:
        // n (on first line) + (n-1) (on second line) + (n-2) on third line + ... + , etc.

        // Parameters of the sequence:
        // reason = (-1)
        // U1 = n
        // n_ = row
        // Un = u1 + (n_-1)*r
        // number of element before reaching the row "x" = (U1 + Un/2)*n_
        let mut row_offset: usize = 0;
        if row > 0 {
            let r: i32 = -1;
            let u_1 = n as i32;
            // works because we start by 0 (array index), so it stops at the right place.
            // to be more correct with the arithmetic sequence sum definition,
            // we have to make n = row + 1 => (as we start by 1) and then, at the end,
            // subtract the result with u_n because we want precedents row only and not our.
            let n_ = row as i32;
            let u_n = u_1 + (n_ - 1) * r;
            // be aware that u_1 + u_n / 2 may be float
            let x = n_ as f64 * ((u_1 as f64 + u_n as f64) / 2.0) as f64;
            row_offset = x as usize
        }
        let col_offset = col - row;
        row_offset + col_offset
    }
    fn in_to_out_index(i: usize, n: usize) -> (usize, usize) {
        // first, we need to find the row we're in
        let mut next_row = 0;
        let mut current_row_index;
        loop {
            // to compensate row beginning by 0 and sticking to
            // the formula
            let next_row_adj = next_row + 1;
            let u_n = n as i32 + (next_row_adj - 1) * -1;

            let x = next_row_adj as f64 * ((n as f64 + u_n as f64) / 2.0);
            // the following conversions are ok because the formula implies that the index is >= 0
            let next_row_index = x as usize;
            current_row_index = next_row_index - u_n as usize;
            next_row += 1;
            if i < next_row_index && i >= current_row_index {
                break;
            }
        }
        // conversion Ok, next_row is > 0
        let row = next_row as usize - 1;
        // now we have the row
        // as the offset = col - row => col = offset + row
        // and offset = i - current_row_index
        let offset = i - current_row_index;
        (row, offset + row)
    }
    pub fn grow_fn<F>(&self, by: usize, mut f: F) -> Self
        where F: FnMut(usize, usize) -> T {
        // create a new matrix of the new size and directly copy the element of this one
        SymMatrix::new_fn(self.n + by, |row, col| {
            if row >= self.n || col >= self.n {
                // this is added row, execute our f
                f(row, col)
            } else {
                // copy our content to the new matrix
                self[(row, col)]
            }
        })
    }

    pub fn remove_id(&self, id: usize) -> Self {
        SymMatrix::new_fn(self.n - 1, |row, col| {
            let row = if row >= id {row + 1} else {row};
            let col = if col >= id {col + 1} else {col};
            self[(row, col)]
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::matrix::SymMatrix;

    #[derive(Copy, Clone)]
    struct DataTest(bool);

    impl DataTest {
        fn new(_: usize, _: usize) -> Self {
            DataTest(false)
        }
        fn set(&mut self, val: bool) {
            self.0 = val;
        }
    }

    fn create_matrix_with(size: usize, val: i32) -> SymMatrix<i32> {
        SymMatrix::new_fn(size, |_, _| val)
    }

    #[test]
    fn create_matrix() {
        let m1 = create_matrix_with(4, 4);
        assert_eq!(m1.n, 4);
        for i in 0..m1.n {
            for j in 0..m1.n {
                assert_eq!(m1[(i, j)], 4)
            }
        }
    }

    #[test]
    fn grow_matrix() {
        let ori_val = 4;
        let new_val = 3;
        if ori_val == new_val {
            panic!("Must not have the same value")
        }
        let m1 = create_matrix_with(4, ori_val);
        let m2 = m1.grow_fn(2, |_, _| new_val);
        for i in 0..m2.size() {
            for j in 0..m2.size() {
                if i >= m1.size() || j >= m1.size() {
                    assert_eq!(m2[(i, j)], new_val)
                } else {
                    assert_eq!(m2[(i, j)], ori_val)
                }
            }
        }
    }

    #[test]
    fn modify_matrix() {
        let mut m1 = SymMatrix::new_fn(4, DataTest::new);
        m1[(2, 2)].set(true);
        assert_eq!(m1[(2, 2)].0, true)
    }

    #[test]
    fn remove_id() {
        let mut val = 0;
        let m1 = SymMatrix::new_fn(4, |_ ,_| {
            val += 1;
            val
        });
        println!("Original {:?}", m1);

        let m2 = m1.remove_id(1);
        println!("Second {:?}", m2);
    }
}