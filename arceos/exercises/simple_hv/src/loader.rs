use std::io::{self, Read};
use std::fs::File;
use axhal::paging::MappingFlags;
use axhal::mem::{PAGE_SIZE_4K, phys_to_virt};
use axmm::AddrSpace;
use crate::VM_ENTRY;

pub fn load_vm_image(fname: &str, uspace: &mut AddrSpace) -> io::Result<()> {
    let mut buf = [0u8; 64];
    load_file(fname, &mut buf)?;

    // 映射客户机代码页
    uspace.map_alloc(VM_ENTRY.into(), PAGE_SIZE_4K, MappingFlags::READ|MappingFlags::WRITE|MappingFlags::EXECUTE|MappingFlags::USER, true).unwrap();

    let (paddr, _, _) = uspace
        .page_table()
        .query(VM_ENTRY.into())
        .unwrap_or_else(|_| panic!("Mapping failed for segment: {:#x}", VM_ENTRY));

    ax_println!("paddr: {:#x}", paddr);

    unsafe {
        core::ptr::copy_nonoverlapping(
            buf.as_ptr(),
            phys_to_virt(paddr).as_mut_ptr(),
            PAGE_SIZE_4K,
        );
    }

    // 映射地址 0，用于存放测试数据
    // 客户机会从地址 0x40 读取数据
    uspace.map_alloc(0.into(), PAGE_SIZE_4K, MappingFlags::READ|MappingFlags::WRITE|MappingFlags::USER, true).unwrap();
    
    let (paddr_zero, _, _) = uspace
        .page_table()
        .query(0.into())
        .unwrap_or_else(|_| panic!("Mapping failed for address 0"));
    
    // 在地址 0x40 处写入测试数据 0x6688
    unsafe {
        let data_ptr = phys_to_virt(paddr_zero).as_mut_ptr() as *mut usize;
        core::ptr::write(data_ptr.add(0x40 / 8), 0x6688);
        core::ptr::write(data_ptr.add(0x48 / 8), 0x1234);
    }
    
    ax_println!("Setup test data at address 0x40: 0x6688");

    Ok(())
}

fn load_file(fname: &str, buf: &mut [u8]) -> io::Result<usize> {
    ax_println!("app: {}", fname);
    let mut file = File::open(fname)?;
    let n = file.read(buf)?;
    Ok(n)
}
